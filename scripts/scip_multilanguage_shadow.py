"""Build strict SCIP/Code IR v1.5 shadow artifacts for the non-Go qeval lanes."""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any


SPIKE_ROOT = Path(__file__).resolve().parents[1]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def make_records(root: Path, code_ir_root: Path, fixtures: Path, languages: list[str]) -> dict[str, Any]:
    result = subprocess.run(
        [
            "node",
            str(SPIKE_ROOT / "scripts/export_syntax_ir_record.mjs"),
            str(root),
            str(code_ir_root / "dist/engine/code-ir/index.js"),
            str(fixtures),
            *languages,
        ],
        check=True,
        capture_output=True,
        text=True,
        cwd=SPIKE_ROOT,
    )
    return json.loads(result.stdout)


def run(root: Path, output: Path, code_ir_root: Path, fixtures: Path, languages: list[str]) -> list[dict[str, Any]]:
    root = root.resolve(strict=True)
    code_ir_root = code_ir_root.resolve(strict=True)
    fixtures = fixtures.resolve(strict=True)
    output = output.resolve()
    if output.exists():
        raise ValueError("SCIP shadow output directory must be new")
    output.mkdir(parents=True)

    records = make_records(root, code_ir_root, fixtures, languages)["snapshots"]
    sys.path.insert(0, str(root))
    sys.path.insert(0, str(SPIKE_ROOT))
    import scip_pb2
    from scripts.lsp_ir_shadow_join import shadow_join
    from scripts.rust_scip_reassessment import verify_inputs as verify_rust_inputs
    from scripts.rust_scip_shadow_import import normalize as normalize_rust
    from scripts.scip_compat_reassessment import normalize as normalize_compat

    outputs = []
    for language in languages:
        ir_record = records[language]
        index_path = root / language / "index.scip"
        index_bytes = index_path.read_bytes()
        index = scip_pb2.Index()
        index.ParseFromString(index_bytes)
        if language == "rust":
            verify_rust_inputs(root, index, ir_record)
            normalized = normalize_rust(root, index)
            producer_counts = normalized.get("dropped_reference_sites", {})
        else:
            normalized, counts = normalize_compat(root, language, index, ir_record, scip_pb2)
            producer_counts = dict(counts)
        enriched, join_counts = shadow_join(
            (root / language).resolve(strict=True), language, normalized, ir_record
        )
        strict_references = sum(
            fact["kind"] == "references" and fact["status"] == "type_resolved" and bool(fact.get("object_id"))
            for fact in enriched["facts"]
        )
        artifact = {
            "schema": "multilanguage-scip-code-ir-shadow-v1",
            "language": language,
            "snapshot": enriched,
            "scip_sha256": hashlib.sha256(index_bytes).hexdigest(),
            "syntax_snapshot_id": ir_record["snapshot"]["snapshot_id"],
            "producer": f"{index.metadata.tool_info.name}@{index.metadata.tool_info.version}",
            "position_encoding": sorted({document.position_encoding for document in index.documents}),
            "strict_reference_facts": strict_references,
            "producer_counts": producer_counts,
            "join_counts": join_counts,
        }
        destination = output / f"{language}-scip-shadow-v15.json"
        payload = json.dumps(artifact, sort_keys=True, separators=(",", ":")) + "\n"
        destination.write_text(payload)
        outputs.append({
            "language": language,
            "producer": artifact["producer"],
            "position_encoding": artifact["position_encoding"],
            "source_snapshot_id": artifact["syntax_snapshot_id"],
            "scip_sha256": artifact["scip_sha256"],
            "shadow_sha256": sha256_file(destination),
            "strict_reference_facts": strict_references,
            "producer_counts": producer_counts,
            "join_counts": join_counts,
            "artifact": str(destination),
        })
    return outputs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--code-ir-root", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("languages", nargs="*", choices=["python", "rust", "typescript"])
    args = parser.parse_args()
    if not args.languages:
        args.languages = ["python", "rust", "typescript"]
    print(json.dumps(run(args.root, args.output, args.code_ir_root, args.fixtures, args.languages), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
