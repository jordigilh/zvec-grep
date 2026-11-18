#!/usr/bin/env python3
"""Retrieval quality lane: run the zg CLI hybrid search over a locked fixture
and score it against the fixture's adjudicated qrels.

Adapted from the engram upstream-sync Tier-1 qeval runner (2026-11-18) with
repo-relative imports: the scorer and normalization are vendored beside this
script so the fork is self-contained.

The runner copies a fixture's source files into a fresh work root, builds the
index once (refresh=wait warmup), then runs the measured pass with refresh=off
so every query sees the same frozen index. Compact CLI output is normalized to
manifest unit IDs and scored with the frozen evaluate_semantic_search scorer.

Usage:
  python3 run_qeval.py --fixture <fixture-dir> --binary <zg> \
      --work-root <dir> --output <dir> --model-cache <dir>
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from evaluate_semantic_search import evaluate  # noqa: E402
from replay_normalization import (  # noqa: E402
    _unit_spans,
    _zvec_results,
    copy_fixture_sources,
)


def run_queries(
    binary: Path, root: Path, queries: list[dict], refresh: str, model_cache: Path
) -> list[dict]:
    rows = []
    for query in queries:
        command = [
            str(binary),
            "--mode", "direct",
            "--refresh", refresh,
            "--limit", "10",
            "--compact",
            "--preview", "full",
            "--model-cache", str(model_cache),
            "--device", "cpu",
            "--hybrid", query["query"],
        ]
        completed = subprocess.run(
            command, cwd=root, capture_output=True, text=True, check=False
        )
        if completed.returncode:
            raise SystemExit(
                f"zg failed for {query['id']}: {completed.stderr.strip()[:2000]}"
            )
        rows.append({
            "id": query["id"],
            "query": query["query"],
            "response": {"stdout": completed.stdout, "stderr": completed.stderr},
        })
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--model-cache",
        type=Path,
        default=Path(os.environ.get("ZVEC_GREP_MODEL_CACHE",
                                    str(Path.home() / ".engram/zvec-grep/model"))),
    )
    args = parser.parse_args()

    fixture = args.fixture.resolve()
    binary = args.binary.resolve()
    model_cache = args.model_cache.resolve()
    output = args.output
    if output.exists():
        raise SystemExit(f"output directory already exists: {output}")

    manifest = json.loads((fixture / "manifest.json").read_text(encoding="utf-8"))
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    qrels = json.loads((fixture / "qrels.json").read_text(encoding="utf-8"))
    queries = truth["queries"]
    units = _unit_spans(fixture, manifest)

    root = args.work_root.resolve()
    copy_fixture_sources(fixture, manifest, root)

    # Build the index once (refresh wait), then run the measured pass with
    # refresh off so every query sees the same frozen index.
    run_queries(binary, root, queries[:1], "wait", model_cache)
    rows = run_queries(binary, root, queries, "off", model_cache)

    with binary.open("rb") as binary_file:
        binary_sha256 = hashlib.file_digest(binary_file, "sha256").hexdigest()

    normalized_queries = [
        {"id": row["id"], "results": _zvec_results(units, row["response"])}
        for row in rows
    ]
    runs = {
        "schema_version": 1,
        "suite_id": truth["suite_id"],
        "fixture_id": manifest["fixture_id"],
        "source": qrels["source"],
        "result_limit": 10,
        "runs": [{"backend": "zvec-cli-hybrid", "queries": normalized_queries}],
    }
    metrics = evaluate(qrels, runs, cutoff=10)

    output.mkdir(parents=True)
    (output / "raw-runs.json").write_text(json.dumps({
        "binary_sha256": binary_sha256,
        "root": str(root),
        "queries": rows,
    }, indent=2) + "\n", encoding="utf-8")
    (output / "normalized-runs.json").write_text(json.dumps(runs, indent=2) + "\n", encoding="utf-8")
    (output / "metrics-k10.json").write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")
    overall = metrics["runs"][0]["overall"]
    print(json.dumps({
        "fixture_id": manifest["fixture_id"],
        "binary_sha256": binary_sha256,
        "ndcg@10": overall["ndcg@10"],
        "mrr@10": overall["mrr@10"],
        "recall@10": overall["recall@10"],
        "precision@10": overall["precision@10"],
    }))


if __name__ == "__main__":
    main()
