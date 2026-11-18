#!/usr/bin/env python3
"""Verify the committed fixtures are byte-identical to their baselines.json
sha256 references. Guards the lanes against accidental fixture edits; a
fixture change must go through the same reviewed re-baselining PR as a
baseline change.

Usage:
  python3 check_fixture_hashes.py [--baselines baselines.json]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baselines", type=Path, default=here / "baselines.json")
    args = parser.parse_args()

    baselines = json.loads(args.baselines.read_text())
    if baselines.get("schema") != "quality-lanes-baselines-v1":
        print("baselines.json: unexpected schema", file=sys.stderr)
        return 2

    problems = []
    for fixture_id, entry in sorted(baselines["fixtures"].items()):
        fixture_dir = here / "fixtures" / fixture_id
        for relative, expected in sorted(entry["sha256"].items()):
            path = fixture_dir / relative
            if not path.is_file():
                problems.append(f"{fixture_id}/{relative}: missing")
                continue
            actual = hashlib.sha256(path.read_bytes()).hexdigest()
            if actual != expected:
                problems.append(
                    f"{fixture_id}/{relative}: expected {expected}, got {actual}"
                )

    if problems:
        print("FIXTURE HASH MISMATCH:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print("OK: all fixtures match their baselines.json sha256 references")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
