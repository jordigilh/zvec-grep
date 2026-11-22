#!/usr/bin/env python3
"""No-regression gate for one multi-language retrieval lane result.

The contract is *equal or better* per language and per metric:

- every metric in ``overall`` must be >= its pinned floor in
  ``baselines-multilanguage.json``;
- an improvement passes and is printed, but must be re-pinned in a reviewed
  change so the floor does not silently go stale;
- a regression fails the lane.

Only representation noise plus cross-platform inference drift (``2e-3`` in
nDCG, observed at 9.5e-4 between macOS and Ubuntu for an identical ranked set)
is tolerated; there is no quality allowance beyond that. Cross-language averages are never computed.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

METRICS = ("ndcg@10", "mrr@10", "recall@10", "precision@10")
# Cross-platform inference drift, not a quality allowance.
REPRESENTATION_EPSILON = 2e-3


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baselines", type=Path, default=here / "baselines-multilanguage.json"
    )
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()

    baselines = json.loads(args.baselines.read_text(encoding="utf-8"))
    if baselines.get("schema") != "quality-lanes-baselines-multilanguage-v1":
        print("unexpected baselines schema", file=sys.stderr)
        return 2

    entry = baselines.get("fixtures", {}).get(args.fixture)
    if entry is None:
        print(f"no pinned floor for fixture {args.fixture}", file=sys.stderr)
        return 2

    payload = json.loads(args.results.read_text(encoding="utf-8"))
    if payload.get("fixture_id") != args.fixture:
        print(
            f"results are for {payload.get('fixture_id')}, not {args.fixture}",
            file=sys.stderr,
        )
        return 2

    pinned_snapshot = entry.get("fixture_snapshot_sha256")
    measured_snapshot = payload.get("fixture", {}).get("snapshot_sha256")
    if pinned_snapshot != measured_snapshot:
        print(
            f"{args.fixture}: fixture snapshot {measured_snapshot} does not match "
            f"pinned {pinned_snapshot}",
            file=sys.stderr,
        )
        return 2

    overall = payload.get("overall", {})
    regressions: list[str] = []
    improvements: list[str] = []
    for metric in METRICS:
        got = overall.get(metric)
        floor = entry["retrieval"].get(metric)
        if not isinstance(got, (int, float)) or not isinstance(floor, (int, float)):
            print(f"{args.fixture}: missing {metric}", file=sys.stderr)
            return 2
        if got + REPRESENTATION_EPSILON < floor:
            regressions.append(
                f"{metric}: {got:.6f} < floor {floor:.6f} ({got - floor:+.6f})"
            )
        elif got > floor + REPRESENTATION_EPSILON:
            improvements.append(
                f"{metric}: {got:.6f} > floor {floor:.6f} ({got - floor:+.6f})"
            )

    if improves := improvements:
        for line in improves:
            print(f"{args.fixture}: improvement {line}")
    if regressions:
        print(f"RETRIEVAL REGRESSION: {args.fixture}", file=sys.stderr)
        for line in regressions:
            print(f"  {line}", file=sys.stderr)
        return 1

    print(f"OK: {args.fixture} meets or beats all four pinned floors")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
