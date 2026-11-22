#!/usr/bin/env python3
"""Check a lane result against baselines.json.

Graph measurements are byte-reproducible: ANY deviation from the baseline —
including an "improvement" — fails the lane, and any change requires
re-baselining through a reviewed PR that changes baselines.json.

Retrieval measurements come from platform-specific inference. Tokenisation and
attention kernels differ enough that an identical ranked set can still differ in
nDCG by roughly 1e-3 across macOS and Ubuntu (observed 9.5e-4 on
go-workflow-discovery-v1 in CI run 45060504745, while MRR, recall and precision
were identical). Only nDCG is therefore compared with a tolerance, far below
any real quality change; MRR, recall and precision are exact because their quanta
are small enough that a tolerance would hide a single missed judgement.

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
# Cross-platform inference drift, not a quality allowance. Only nDCG drifted
# between macOS and Ubuntu for an identical ranked set; MRR, recall and
# precision were identical, and their quanta are small enough that a tolerance
# would hide a single missed judgement. See module docstring.
NDCG_TOLERANCE = 2e-3
GRAPH_KEYS = [
    "edge_count",
    "verdicts",
    "failure_modes",
    "direction_flips",
    "compiler_truth_pairs",
    "compiler_pairs_present",
    "compiler_pairs_missed",
    "resolved_pending_split",
    # Full verdict-set equality: the exact confirmed / refuted pair sets and
    # the exact pending-external list, so composition drift fails even when
    # aggregate counts are unchanged.
    "confirmed_pairs",
    "refuted_pairs",
    "pending_externals",
]


def diff(
    path: str,
    expected,
    actual,
    problems: list[str],
    tolerance: float = 0.0,
) -> None:
    if isinstance(expected, float) or isinstance(actual, float):
        if tolerance and isinstance(expected, (int, float)) and isinstance(actual, (int, float)):
            within = abs(float(actual) - float(expected)) <= tolerance
        else:
            within = expected == actual
        if not within:
            problems.append(f"{path}: expected {expected!r}, got {actual!r}")
    elif expected != actual:
        problems.append(f"{path}: expected {expected!r}, got {actual!r}")


def missed_pair_key(pair: dict) -> tuple:
    return (pair["caller"], pair["callee"], pair["caller_file"], pair["callee_file"])


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
            tolerance = NDCG_TOLERANCE if key == "ndcg@10" else 0.0
            diff(
                f"overall.{key}",
                fixture["retrieval"][key],
                overall[key],
                problems,
                tolerance,
            )
    else:
        problems = []
        for key in GRAPH_KEYS:
            if key == "compiler_pairs_missed":
                # The adjudicator builds the missed list from a set, so its
                # order is not stable across runs; compare the pairs
                # canonically sorted, keeping exact record equality.
                expected = fixture["graph"][key]
                actual = results.get(key)
                if (
                    not isinstance(expected, list)
                    or not isinstance(actual, list)
                    or sorted(expected, key=missed_pair_key)
                    != sorted(actual, key=missed_pair_key)
                ):
                    diff(key, expected, actual, problems)
            else:
                diff(key, fixture["graph"][key], results.get(key), problems)

    if problems:
        print(f"QUALITY REGRESSION on {args.fixture} ({args.lane} lane):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        if args.lane == "retrieval":
            print(
                "Retrieval metrics must match the pin (nDCG within 2e-3 for "
                "cross-platform drift, the rest exactly). Re-baseline via a "
                "reviewed PR that updates baselines.json.",
                file=sys.stderr,
            )
        else:
            print(
                "Any graph deviation — including improvements — fails this lane. "
                "Re-baseline via a reviewed PR that updates baselines.json.",
                file=sys.stderr,
            )
        return 1

    if args.lane == "retrieval":
        print(
            f"OK: {args.fixture} retrieval lane matches baselines.json "
            f"(nDCG within {NDCG_TOLERANCE:g})"
        )
    else:
        print(f"OK: {args.fixture} graph lane matches baselines.json exactly")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
