#!/usr/bin/env python3
"""Verify the four-language fixtures match their pinned snapshots.

Byte-level pinning of every vendored lane file is ``check_fixture_hashes.py``'s
job.  This check covers the semantic contract instead: each fixture must carry
``manifest.json``, ``truth.json`` and ``qrels.json``; the qrels source digest
must equal ``baselines-multilanguage.json``'s pinned snapshot; and truth,
qrels and manifest must agree on the query and unit sets.

Run from the lane directory or pass ``--baselines`` explicitly.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baselines",
        type=Path,
        default=here / "baselines-multilanguage.json",
    )
    parser.add_argument("--fixtures", type=Path, default=here / "fixtures")
    args = parser.parse_args()

    baselines = json.loads(args.baselines.read_text(encoding="utf-8"))
    if baselines.get("schema") != "quality-lanes-baselines-multilanguage-v1":
        print("unexpected baselines schema", file=sys.stderr)
        return 2

    problems: list[str] = []
    for fixture_id, entry in sorted(baselines["fixtures"].items()):
        root = args.fixtures / fixture_id
        paths = {
            name: root / name
            for name in ("manifest.json", "truth.json", "qrels.json")
        }
        missing = [name for name, path in paths.items() if not path.is_file()]
        if missing:
            problems.append(f"{fixture_id}: missing {', '.join(missing)}")
            continue

        manifest = json.loads(paths["manifest.json"].read_text(encoding="utf-8"))
        truth = json.loads(paths["truth.json"].read_text(encoding="utf-8"))
        qrels = json.loads(paths["qrels.json"].read_text(encoding="utf-8"))

        pinned = entry.get("fixture_snapshot_sha256")
        actual = qrels.get("source", {}).get("snapshot_sha256")
        if pinned != actual:
            problems.append(
                f"{fixture_id}: snapshot {actual} does not match pinned {pinned}"
            )

        unit_ids = {unit["unit_id"] for unit in manifest["units"]}
        if qrels.get("unit_count") != len(unit_ids):
            problems.append(
                f"{fixture_id}: qrels unit_count {qrels.get('unit_count')} "
                f"!= manifest units {len(unit_ids)}"
            )

        truth_ids = [query["id"] for query in truth["queries"]]
        qrels_ids = [query["id"] for query in qrels["queries"]]
        if truth_ids != qrels_ids:
            problems.append(f"{fixture_id}: truth and qrels query order differ")

        for query in qrels["queries"]:
            judged = {judgment["unit_id"] for judgment in query["judgments"]}
            unknown = judged - unit_ids
            if unknown:
                problems.append(
                    f"{fixture_id}/{query['id']}: unjudged unit ids "
                    f"{', '.join(sorted(unknown)[:3])}"
                )

    if problems:
        print("MULTI-LANGUAGE FIXTURE CHECK FAILED:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    print(
        f"OK: {len(baselines['fixtures'])} multi-language fixtures match "
        "their pinned snapshots and query/unit sets"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
