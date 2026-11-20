#!/usr/bin/env python3
"""Render the multi-language lane's JSONL output as a markdown table.

The lane is report-only: it prints one JSON object per fixture and this script
turns those lines into a table for ``$GITHUB_STEP_SUMMARY``.  It never compares
against a baseline, so a metric change is visible but not fatal.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <summary.jsonl>", file=sys.stderr)
        return 2

    path = Path(argv[1])
    if not path.is_file():
        print("no summary produced", file=sys.stderr)
        return 0

    print("| fixture | nDCG@10 | MRR@10 | Recall@10 | Precision@10 | unmatched | deterministic |")
    print("|---|---:|---:|---:|---:|---:|---|")
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        row = json.loads(line)
        print(
            f"| {row['fixture_id']} | {row['ndcg@10']:.4f} | {row['mrr@10']:.4f} | "
            f"{row['recall@10']:.4f} | {row['precision@10']:.4f} | "
            f"{row['unmatched_hits']} | {'yes' if row['deterministic'] else 'no'} |"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
