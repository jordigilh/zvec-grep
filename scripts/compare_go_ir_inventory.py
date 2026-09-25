"""Compare Go Code IR declarations with independent go/parser source spans."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


def compare(fixture: Path, inventory_path: Path, ir_path: Path) -> dict[str, Any]:
    fixture = fixture.resolve()
    inventory = json.loads(inventory_path.read_text())
    ir_data = json.loads(ir_path.read_text())
    snapshot = ir_data.get("snapshot", ir_data)
    if inventory.get("schema") != "go-source-unit-inventory-v1":
        raise ValueError("unsupported Go source inventory")
    if snapshot.get("schema") != "zvec-grep.code-ir" or snapshot.get("schema_version") != 1:
        raise ValueError("unsupported Code IR snapshot")

    file_paths = {file["file_id"]: file["relative_path"] for file in snapshot["files"]}
    sources = {}
    for file in snapshot["files"]:
        raw = (fixture / file["relative_path"]).read_bytes()
        if hashlib.sha256(raw).hexdigest() != file["sha256"]:
            raise ValueError(f"Code IR source digest differs: {file['relative_path']}")
        sources[file["file_id"]] = raw

    inventory_digest = source_set_digest(
        fixture, {row["path"] for row in inventory["units"]}
    )
    if inventory["source_set_sha256"] != inventory_digest:
        raise ValueError("independent inventory source-set digest differs")
    if inventory["source_set_sha256"] != snapshot_source_digest(snapshot, fixture):
        raise ValueError("Code IR selected file set differs from the independent inventory")

    rows = []
    for expected in inventory["units"]:
        expected_name = expected["symbol"].split(".")[-1]
        candidates = [
            unit
            for unit in snapshot["units"]
            if unit.get("name") == expected_name
            and unit.get("kind") == expected["kind"]
            and file_paths.get(unit["source"]["file_id"]) == expected["path"]
        ]
        if len(candidates) != 1:
            rows.append({
                "unit_id": expected["unit_id"],
                "status": "missing" if not candidates else "ambiguous",
                "candidates": len(candidates),
            })
            continue

        actual = candidates[0]
        source = actual["source"]
        raw = sources[source["file_id"]]
        name_start = expected["identifier_start_byte"]
        name_end = expected["identifier_end_byte"]
        token_matches = (
            raw[name_start:name_end].decode("utf-8") == expected_name
            and source["start_byte"] <= name_start < name_end <= source["end_byte"]
        )
        identifier_ref = (actual.get("extensions") or {}).get("data", {}).get("identifier_source")
        identifier_ref_matches = identifier_ref is None or (
            identifier_ref["start_byte"] == name_start
            and identifier_ref["end_byte"] == name_end
            and identifier_ref["sha256"] == source["sha256"]
        )
        span_matches = (
            source["start_byte"] == expected["start_byte"]
            and source["end_byte"] == expected["end_byte"]
        )
        exact = span_matches and token_matches and identifier_ref_matches
        rows.append({
            "unit_id": expected["unit_id"],
            "status": "exact" if exact else "mismatch",
            "expected": [expected["start_byte"], expected["end_byte"], expected["kind"]],
            "actual": [source["start_byte"], source["end_byte"], actual["kind"]],
            "identifier_token_matches": token_matches,
            "identifier_source_matches": identifier_ref_matches,
        })

    counts = {
        status: sum(row["status"] == status for row in rows)
        for status in ("exact", "mismatch", "missing", "ambiguous")
    }
    return {
        "schema": "go-code-ir-independent-inventory-comparison-v1",
        "source_set_sha256": inventory["source_set_sha256"],
        "frontend": snapshot["frontend_versions"].get("go"),
        "inventory_units": len(inventory["units"]),
        "code_ir_units": len(snapshot["units"]),
        "exact_span_kind_and_token_matches": counts["exact"],
        "counts": counts,
        "units": rows,
    }


def source_set_digest(root: Path, relative_paths: set[str]) -> str:
    digest = hashlib.sha256()
    for relative in sorted(relative_paths):
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update((root / relative).read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def snapshot_source_digest(snapshot: dict[str, Any], root: Path) -> str:
    return source_set_digest(
        root, {file["relative_path"] for file in snapshot["files"]}
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--ir", type=Path, required=True)
    args = parser.parse_args()
    result = compare(args.fixture, args.inventory, args.ir)
    print(json.dumps(result, indent=2))
    return 0 if result["counts"]["exact"] == result["inventory_units"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
