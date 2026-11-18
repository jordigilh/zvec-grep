#!/usr/bin/env python3
"""Check a lane result against baselines.json with exact equality.

The quality lanes gate on byte-reproducible measurements: every measurement to
date has been identical across machines, so ANY deviation from the baseline —
including an "improvement" — fails the lane. Improvements (and acknowledged
regressions) require re-baselining through a reviewed PR that changes
baselines.json.

Usage:
  python3 check_baselines.py --baselines baselines.json --lane retrieval \
      --fixture go-workflow-discovery-v1 --results metrics-k10.json

  python3 check_baselines.py --baselines baselines.json --lane graph \
      --fixture go-workflow-discovery-v1 --results adjudication.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

RETRIEVAL_KEYS = ["ndcg@10", "mrr@10", "recall@10", "precision@10"]
GRAPH_KEYS = [
    "edge_count",
    "verdicts",
    "failure_modes",
    "direction_flips",
    "compiler_truth_pairs",
    "compiler_pairs_present",
    "compiler_pairs_missed",
    "resolved_pending_split",
]


def diff(path: str, expected, actual, problems: list[str]) -> None:
    if isinstance(expected, float) or isinstance(actual, float):
        if expected != actual:  # exact float equality is the contract
            problems.append(f"{path}: expected {expected!r}, got {actual!r}")
    elif expected != actual:
        problems.append(f"{path}: expected {expected!r}, got {actual!r}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baselines", type=Path, required=True)
    parser.add_argument("--lane", choices=["retrieval", "graph"], required=True)
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()

    baselines = json.loads(args.baselines.read_text())
    if baselines.get("schema") != "quality-lanes-baselines-v1":
        print("baselines.json: unexpected schema", file=sys.stderr)
        return 2
    fixture = baselines["fixtures"].get(args.fixture)
    if fixture is None:
        print(f"baselines.json: no entry for fixture {args.fixture}", file=sys.stderr)
        return 2

    results = json.loads(args.results.read_text())

    if args.lane == "retrieval":
        overall = results["runs"][0]["overall"]
        problems = []
        for key in RETRIEVAL_KEYS:
            diff(f"overall.{key}", fixture["retrieval"][key], overall[key], problems)
    else:
        problems = []
        for key in GRAPH_KEYS:
            diff(key, fixture["graph"][key], results.get(key), problems)

    if problems:
        print(f"QUALITY REGRESSION on {args.fixture} ({args.lane} lane):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "Any deviation — including improvements — fails this lane. "
            "Re-baseline via a reviewed PR that updates baselines.json.",
            file=sys.stderr,
        )
        return 1

    print(f"OK: {args.fixture} {args.lane} lane matches baselines.json exactly")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
