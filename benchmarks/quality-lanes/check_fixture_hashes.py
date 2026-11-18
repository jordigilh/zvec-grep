#!/usr/bin/env python3
"""Verify every vendored file under benchmarks/quality-lanes/ is
byte-identical to its baselines.json sha256 reference.

Pins every file in the lane tree — fixture sources, query suites, compiler
truth, all scripts, READMEs, and the go-graph-truth regeneration tool —
except baselines.json itself (it holds the pins and cannot contain its own
hash). This includes check_fixture_hashes.py: editing the checker changes
its pinned hash and fails the check until a reviewed PR re-generates the
pins. Editing any pinned file must present as a hash error, not as a
confusing "quality regression" in a lane.

Regeneration (the re-baselining lever for asset edits):

  python3 check_fixture_hashes.py --regenerate

rewrites the ``files`` map in baselines.json from the working tree. Run it
only as part of a reviewed change that intentionally edits lane assets.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

EXCLUDED = {"baselines.json"}


def pinned_files(root: Path) -> list[Path]:
    return sorted(
        path
        for path in root.rglob("*")
        if path.is_file()
        and "__pycache__" not in path.parts
        and path.relative_to(root).as_posix() not in EXCLUDED
    )


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baselines", type=Path, default=here / "baselines.json")
    parser.add_argument(
        "--regenerate",
        action="store_true",
        help="rewrite the files pin map in baselines.json from the working tree",
    )
    args = parser.parse_args()

    baselines = json.loads(args.baselines.read_text())
    if baselines.get("schema") != "quality-lanes-baselines-v1":
        print("baselines.json: unexpected schema", file=sys.stderr)
        return 2

    actual = {
        path.relative_to(here).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in pinned_files(here)
    }

    if args.regenerate:
        baselines["files"] = actual
        args.baselines.write_text(json.dumps(baselines, indent=2) + "\n")
        print(f"regenerated pin map: {len(actual)} files")
        return 0

    expected = baselines.get("files", {})

    problems = []
    for relative in sorted(set(expected) | set(actual)):
        if relative not in actual:
            problems.append(f"{relative}: pinned but missing from the tree")
        elif relative not in expected:
            problems.append(f"{relative}: present but not pinned (run --regenerate)")
        elif expected[relative] != actual[relative]:
            problems.append(
                f"{relative}: expected {expected[relative]}, got {actual[relative]}"
            )

    if problems:
        print("LANE ASSET HASH MISMATCH:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "Vendored assets must not drift; re-pin only via a reviewed PR "
            "(check_fixture_hashes.py --regenerate).",
            file=sys.stderr,
        )
        return 1
    print(f"OK: all {len(actual)} lane files match their baselines.json sha256 pins")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
