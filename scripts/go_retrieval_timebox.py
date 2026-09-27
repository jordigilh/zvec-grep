#!/usr/bin/env python3
"""Go-only syntax discovery diagnosis; source/qrels are verified and read-only."""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

from replay_code_ir_v2_qeval import (
    REPOSITORY, compare_metrics, load_engram, normalize_arm, sha256_file, stage_sources, tree_digest,
)


def validate_reranked(pool, reranked):
    if set(pool) != set(reranked):
        raise ValueError("reranked query IDs differ from candidate pool")
    for query_id, hits in reranked.items():
        if len(hits) != 10 or [row.get("rank") for row in hits] != list(range(1, 11)):
            raise ValueError(f"reranker must return exactly ten ranked hits: {query_id}")
        indexed = {row["entity_id"]: row for row in pool[query_id]}
        if len(indexed) != len(pool[query_id]) or len({row.get("entity_id") for row in hits}) != 10:
            raise ValueError("duplicate candidate identity")
        for row in hits:
            original = indexed.get(row["entity_id"])
            if original is None or any(row.get(key) != original.get(key) for key in (
                "path", "start_line", "end_line", "entity_id",
            )):
                raise ValueError("reranker invented or moved an indexed source hit")


def positive_rank_inventory(queries, indexed, baseline, pool):
    """Diagnostic source-unit line-overlap inventory, never a ranking input."""
    ids = [row["id"] for row in queries]
    if (len(ids) != len(set(ids)) or set(ids) != set(baseline) or set(ids) != set(pool)):
        raise ValueError("inventory query IDs differ from frozen truth")
    eligible = set(indexed)
    result = {}
    for query in queries:
        key = query["id"]
        ten, thirty = baseline[key], pool[key]
        if len(ten) != len(set(ten)) or len(thirty) != len(set(thirty)):
            raise ValueError("duplicate source unit in candidate pool")
        if not set(ten).issubset(eligible) or not set(thirty).issubset(eligible):
            raise ValueError("ranked unit missing from indexed source")
        positives = [row["unit_id"] for row in query["judgments"] if row["grade"] > 0]
        if len(positives) != len(set(positives)):
            raise ValueError("duplicate positive truth")
        result[key] = []
        for unit in positives:
            rank10 = ten.index(unit) + 1 if unit in ten else None
            rank30 = thirty.index(unit) + 1 if unit in thirty else None
            category = (
                "in_baseline_top_10" if rank10 is not None else
                "not_eligible_in_index" if unit not in eligible else
                "new_in_pool" if rank30 is not None else
                "not_in_top_30"
            )
            result[key].append({"unit_id": unit, "grade": next(
                row["grade"] for row in query["judgments"] if row["unit_id"] == unit
            ), "baseline_rank": rank10, "pool_rank": rank30, "category": category})
    return result


def run(args):
    fixture = args.fixture.resolve(strict=True)
    engram = args.engram_root.resolve(strict=True)
    cache = args.model_cache.resolve(strict=True)
    work, output = args.work_dir.resolve(), args.output_dir.resolve()
    if (any(path.exists() or any(path.is_relative_to(root) for root in (REPOSITORY, engram, fixture))
            for path in (work, output)) or work == output or
            work.is_relative_to(output) or output.is_relative_to(work)):
        raise ValueError("fresh separate work/output paths required outside both checkouts")
    snapshot, build_qrels, evaluate, dedupe, map_chunk, unit_spans = load_engram(engram)
    manifest = json.loads((fixture / "manifest.json").read_text())
    truth = json.loads((fixture / "truth.json").read_text())
    qrels = json.loads((fixture / "qrels.json").read_text())
    if (manifest.get("fixture_id") != "go-workflow-discovery-v1" or manifest.get("language") != "go" or
            len(truth["queries"]) != 8 or [row["id"] for row in truth["queries"]] !=
            [row["id"] for row in qrels["queries"]] or build_qrels(fixture) != qrels):
        raise ValueError("Go frozen source/truth/qrels differ")
    digest, selected, source_bytes = snapshot(fixture, manifest)
    if (digest, len(selected), source_bytes) != (qrels["source"]["snapshot_sha256"],
                                               qrels["source"]["files"], qrels["source"]["bytes"]):
        raise ValueError("source set changed")
    cache_before = tree_digest(cache)
    work.mkdir()
    output.mkdir()
    paths = stage_sources(fixture, manifest, (work,), source_bytes)
    if len(paths) != len(selected):
        raise ValueError("source file count changed")
    raw_path = output / "raw-runs.json"
    command = [
        "node", str(REPOSITORY / "scripts/go_retrieval_timebox_search.mjs"),
        str(fixture), str(work), str(cache), args.model, str(raw_path),
    ]
    if args.rerank:
        command.append("--rerank")
    subprocess.run(command, check=True, cwd=REPOSITORY)
    raw = json.loads(raw_path.read_text())
    if (raw["provenance"]["source_set_sha256"] != digest or
            raw["provenance"]["model_identity"] != args.model or
            [arm["name"] for arm in raw["arms"]] !=
            (["syntax-10", "syntax-30", "go-source-rerank"] if args.rerank
             else ["syntax-10", "syntax-30"]) or
            any([row["id"] for row in arm["queries"]] !=
                [row["id"] for row in truth["queries"]] or
                [row["query"] for row in arm["queries"]] !=
                [row["query"] for row in truth["queries"]] for arm in raw["arms"])):
        raise ValueError("search runs differ from verified fixture")
    if args.rerank:
        validate_reranked(
            {row["id"]: row["hits"] for row in raw["arms"][1]["queries"]},
            {row["id"]: row["hits"] for row in raw["arms"][2]["queries"]},
        )
    units = unit_spans(fixture, manifest)
    normalized = {
        "schema_version": 1, "suite_id": truth["suite_id"],
        "fixture_id": manifest["fixture_id"],
        "source": {**qrels["source"], "fixture": manifest["fixture_id"]},
        "result_limit": 10,
        "runs": [normalize_arm(arm, units, map_chunk, dedupe) for arm in raw["arms"]],
    }
    metrics = evaluate(qrels, normalized, cutoff=10)
    expected = {"ndcg@10": 0.5887735493745122, "mrr@10": 0.7333333333333333,
                "recall@10": 0.6770833333333334, "precision@10": 0.25}
    if any(abs(metrics["runs"][0]["overall"][key] - value) > 1e-6 for key, value in expected.items()):
        raise ValueError("syntax control differs from frozen Sense-enhanced baseline")
    def mapped(hits):
        return list(dict.fromkeys(unit for hit in hits
                                  for unit in map_chunk(units, hit["path"], hit["start_line"], hit["end_line"])))
    indexed = mapped(raw["indexed"])
    first = {row["id"]: mapped(row["hits"]) for row in raw["arms"][0]["queries"]}
    thirty = {row["id"]: mapped(row["hits"]) for row in raw["arms"][1]["queries"]}
    inventory = positive_rank_inventory(qrels["queries"], indexed, first, thirty)
    comparisons = {run["backend"]: compare_metrics({"runs": [metrics["runs"][0], run]}, "go")
                   for run in metrics["runs"][1:]}
    for path, value in ((output / "normalized-runs.json", normalized),
                        (output / "metrics-k10.json", metrics),
                        (output / "positive-inventory.json", inventory),
                        (output / "comparisons.json", comparisons)):
        path.write_text(json.dumps(value, indent=2) + "\n")
    cache_after = tree_digest(cache)
    if cache_before != cache_after:
        raise ValueError("model cache changed during diagnostic")
    run_manifest = {
        "schema": "go-retrieval-diagnostic-manifest-v1",
        "source_set_sha256": digest, "fixture_id": manifest["fixture_id"],
        "engine_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPOSITORY, text=True).strip(),
        "model": args.model, "model_cache_tree_sha256": cache_after,
        "fixture_sha256": {name: sha256_file(fixture / name) for name in ("manifest.json", "truth.json", "qrels.json")},
        "implementation_sha256": {name: sha256_file(REPOSITORY / "scripts" / name) for name in (
            "go_retrieval_timebox.py", "go_retrieval_timebox_search.mjs",
            "go_retrieval_rerank.mjs", "replay_code_ir_v2_qeval.py")},
        "scorer_sha256": sha256_file(engram / "scripts/evaluate_semantic_search.py"),
        "mapping_sha256": sha256_file(engram / "scripts/replay_synthetic_semantic_search.py"),
        "outputs": {name: sha256_file(output / name) for name in (
            "raw-runs.json", "normalized-runs.json", "metrics-k10.json",
            "positive-inventory.json", "comparisons.json")},
    }
    (output / "run-manifest.json").write_text(json.dumps(run_manifest, indent=2) + "\n")
    return {"baseline": metrics["runs"][0]["overall"],
            "pool_top_ten": metrics["runs"][1]["overall"],
            "reranked": metrics["runs"][2]["overall"] if args.rerank else None,
            "comparisons": comparisons,
            "categories": {label: sum(row["category"] == label for rows in inventory.values() for row in rows)
                           for label in ("in_baseline_top_10", "new_in_pool", "not_in_top_30", "not_eligible_in_index")},
            "output_dir": str(output)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("fixture", "engram-root", "model-cache", "work-dir", "output-dir"):
        parser.add_argument(f"--{key}", type=Path, required=True)
    parser.add_argument("--model", default="local/potion-code-16m-v2")
    parser.add_argument("--rerank", action="store_true", help="one-shot frozen Go source-evidence reranker")
    print(json.dumps(run(parser.parse_args()), indent=2))


if __name__ == "__main__":
    main()
