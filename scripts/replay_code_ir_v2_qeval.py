#!/usr/bin/env python3
"""Compare syntax-only retrieval with published Code IR v2 and opt-in ablations.

Engram frozen fixtures/scorer are read-only; outputs and indexes must be new
and outside both source checkouts. No SCIP inputs or default search changes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


REPOSITORY = Path(__file__).resolve().parents[1]
METRICS = ("ndcg@10", "mrr@10", "recall@10", "precision@10")
FACTORIAL_ARMS = (
    "syntax-only", "code-ir-v1", "code-ir-policy-only",
    "code-ir-metadata-only", "code-ir-v2",
)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file() and not p.is_symlink()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(bytes.fromhex(sha256_file(path)))
        digest.update(b"\0")
    return digest.hexdigest()


def load_engram(engram_root: Path):
    sys.path.insert(0, str(engram_root))
    from scripts.build_synthetic_qrels import _snapshot, build_qrels
    from scripts.evaluate_semantic_search import evaluate
    from scripts.replay_synthetic_semantic_search import (
        _dedupe_results, _map_chunk, _unit_spans,
    )

    return _snapshot, build_qrels, evaluate, _dedupe_results, _map_chunk, _unit_spans


def validate_arms(
    arms: list[dict[str, Any]], query_ids: list[str], query_texts: list[str] | None = None,
    expected_names: tuple[str, ...] | list[str] = ("syntax-only", "code-ir-v2"),
) -> None:
    if len(arms) != len(expected_names) or [arm.get("name") for arm in arms] != list(expected_names):
        raise ValueError(f"expected exactly these qeval arms: {expected_names}")
    if len(query_ids) != 8 or len(set(query_ids)) != 8:
        raise ValueError("expected eight unique frozen queries")
    for arm in arms:
        if [row.get("id") for row in arm.get("queries", [])] != query_ids:
            raise ValueError(f"{arm['name']}: raw query IDs/order differ from frozen truth")
        if query_texts is not None and [row.get("query") for row in arm["queries"]] != query_texts:
            raise ValueError(f"{arm['name']}: raw query text differs from frozen truth")


def normalize_arm(arm: dict[str, Any], units, map_chunk, dedupe_results):
    queries = []
    for row in arm["queries"]:
        ranked = []
        for rank, hit in enumerate(row["hits"], 1):
            if hit.get("rank") != rank:
                raise ValueError(f"{arm['name']}: non-contiguous raw rank for {row['id']}")
            start, end = hit.get("start_line"), hit.get("end_line")
            if not isinstance(start, int) or not isinstance(end, int) or start < 1 or end < start:
                raise ValueError(f"{arm['name']}: invalid source span for {row['id']}")
            mapped = map_chunk(units, hit["path"], start, end)
            if not mapped:
                raise ValueError(f"{arm['name']}: unmapped raw hit for {row['id']}")
            ranked.append((rank, mapped))
        if not ranked:
            raise ValueError(f"{arm['name']}: no ranked hits for {row['id']}")
        queries.append({"id": row["id"], "results": dedupe_results(ranked)})
    return {"backend": arm["name"], "queries": queries}


def compare_metrics(metrics: dict[str, Any], language: str) -> dict[str, Any]:
    baseline, candidate = metrics["runs"]
    left, right = baseline["per_query"], candidate["per_query"]
    left_ids = [row["id"] for row in left]
    by_id = {row["id"]: row for row in right}
    if len(left_ids) != 8 or len(set(left_ids)) != 8 or len(by_id) != 8 or set(left_ids) != set(by_id):
        raise ValueError("scored query IDs differ between arms")
    deltas = {key: candidate["overall"][key] - baseline["overall"][key] for key in METRICS}
    per_query = [
        {"id": row["id"], **{key: by_id[row["id"]][key] - row[key] for key in METRICS}}
        for row in left
    ]
    no_regression = all(value >= -1e-12 for value in deltas.values())
    improvement = any(value > 1e-12 for value in deltas.values())
    return {
        "same_engine": True,
        "language": language,
        "baseline": baseline["backend"],
        "candidate": candidate["backend"],
        "aggregate_deltas": deltas,
        "no_regression_gate": no_regression,
        "improvement_gate": improvement,
        "enablement_gate": no_regression and improvement,
        "per_query_deltas": per_query,
        "query_losses": [row for row in per_query if any(row[key] < -1e-12 for key in METRICS)],
    }


def compare_ablation_metrics(metrics: dict[str, Any], language: str) -> dict[str, Any]:
    runs = metrics["runs"]
    if [run["backend"] for run in runs] != list(FACTORIAL_ARMS):
        raise ValueError("factorial scorer arm names/order differ")
    indexed = {run["backend"]: run for run in runs}

    def pair(left: str, right: str) -> dict[str, Any]:
        return compare_metrics({"runs": [indexed[left], indexed[right]]}, language)

    return {
        "language": language,
        "vs_control": {name: pair("syntax-only", name) for name in FACTORIAL_ARMS[1:]},
        "factor_effects": {
            "policy_with_v1_text": pair("code-ir-v1", "code-ir-policy-only"),
            "metadata_with_v1_units": pair("code-ir-v1", "code-ir-metadata-only"),
            "metadata_with_v2_units": pair("code-ir-policy-only", "code-ir-v2"),
            "policy_with_v2_text": pair("code-ir-metadata-only", "code-ir-v2"),
        },
    }


def stage_sources(fixture: Path, manifest: dict[str, Any], roots: tuple[Path, Path], expected_bytes: int):
    paths = sorted({unit["path"] for unit in manifest["units"]})
    selected = {relative: (fixture / relative).read_bytes() for relative in paths}
    if sum(len(raw) for raw in selected.values()) != expected_bytes:
        raise ValueError("frozen source bytes differ from qrels")
    for relative in paths:
        for root in roots:
            destination = root / relative
            if destination.exists():
                raise ValueError(f"staged source already exists: {destination}")
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(selected[relative])
    return paths


def run(args: argparse.Namespace) -> dict[str, Any]:
    fixture = args.fixture.resolve(strict=True)
    engram_root = args.engram_root.resolve(strict=True)
    model_cache = args.model_cache.resolve(strict=True)
    work_dir, output_dir = args.work_dir.resolve(), args.output_dir.resolve()
    for target in (work_dir, output_dir):
        if target.exists() or any(target.is_relative_to(root) for root in (REPOSITORY, engram_root, fixture)):
            raise ValueError("work/output must be fresh directories outside both checkouts")
    if work_dir == output_dir or work_dir.is_relative_to(output_dir) or output_dir.is_relative_to(work_dir):
        raise ValueError("work and output directories must be separate")

    snapshot, build_qrels, evaluate, dedupe, map_chunk, unit_spans = load_engram(engram_root)
    manifest = json.loads((fixture / "manifest.json").read_text())
    truth = json.loads((fixture / "truth.json").read_text())
    qrels = json.loads((fixture / "qrels.json").read_text())
    language = manifest.get("language")
    if language not in {"go", "python", "rust", "typescript"} or manifest.get("fixture_id") != f"{language}-workflow-discovery-v1":
        raise ValueError("unsupported frozen language/fixture ID")
    query_ids = [row["id"] for row in truth["queries"]]
    if len(query_ids) != 8 or len(set(query_ids)) != 8 or query_ids != [row["id"] for row in qrels["queries"]]:
        raise ValueError("fixture truth and qrels query IDs differ")
    if build_qrels(fixture) != qrels:
        raise ValueError("frozen source, manifest, truth and qrels disagree")
    source_digest, selected, source_bytes = snapshot(fixture, manifest)
    if (source_digest, len(selected), source_bytes) != (
        qrels["source"]["snapshot_sha256"], qrels["source"]["files"], qrels["source"]["bytes"]
    ):
        raise ValueError("frozen source snapshot differs from qrels")

    work_dir.mkdir()
    output_dir.mkdir()
    baseline, candidate = work_dir / "baseline", work_dir / "candidate"
    baseline.mkdir()
    candidate.mkdir()
    paths = stage_sources(fixture, manifest, (baseline, candidate), source_bytes)
    if len(paths) != len(selected):
        raise ValueError("staged file count differs from frozen inventory")
    raw_path = output_dir / "raw-runs.json"
    command = [
        "node", str(REPOSITORY / "scripts/qeval_code_ir_v2_search.mjs"),
        "--fixture", str(fixture), "--baseline-root", str(baseline),
        "--candidate-root", str(candidate), "--sidecar-root", str(work_dir / "ir-sidecar"),
        "--index-root", str(work_dir / "indexes"), "--model-cache", str(model_cache),
        "--model", args.model, "--output", str(raw_path),
    ]
    if args.ablation:
        command.append("--ablation")
    subprocess.run(command, check=True, cwd=REPOSITORY)
    raw = json.loads(raw_path.read_text())
    validate_arms(
        raw["arms"], query_ids, [row["query"] for row in truth["queries"]],
        expected_names=FACTORIAL_ARMS if args.ablation else ("syntax-only", "code-ir-v2"),
    )
    if raw["provenance"]["source_set_sha256"] != source_digest or raw["provenance"]["language"] != language:
        raise ValueError("search adapter did not use pinned source/language")
    code_ir = raw["provenance"]["code_ir"]
    if (
        code_ir["projection_version"] != 2
        or code_ir["projection_policy"] != "source-metadata-entity-v1"
        or raw["provenance"]["model_identity"] != args.model
        or raw["provenance"].get("scip") is not None
    ):
        raise ValueError("search adapter did not use requested v2 projection/model without SCIP")
    normalized = {
        "schema_version": 1, "suite_id": truth["suite_id"],
        "fixture_id": manifest["fixture_id"],
        "source": {**qrels["source"], "fixture": manifest["fixture_id"]},
        "result_limit": 10,
        "runs": [normalize_arm(arm, unit_spans(fixture, manifest), map_chunk, dedupe) for arm in raw["arms"]],
    }
    metrics = evaluate(qrels, normalized, cutoff=10)
    comparison = compare_ablation_metrics(metrics, language) if args.ablation else compare_metrics(metrics, language)
    normalized_path = output_dir / "normalized-runs.json"
    metrics_path = output_dir / "metrics-k10.json"
    comparison_path = output_dir / "comparison.json"
    for path, value in ((normalized_path, normalized), (metrics_path, metrics), (comparison_path, comparison)):
        path.write_text(json.dumps(value, indent=2) + "\n")
    manifest_path = output_dir / "run-manifest.json"
    manifest_path.write_text(json.dumps({
        "schema": "code-ir-v2-qeval-manifest-v1", "fixture_id": manifest["fixture_id"],
        "language": language, "source_set_sha256": source_digest, "files": len(paths),
        "manifest_sha256": sha256_file(fixture / "manifest.json"),
        "truth_sha256": sha256_file(fixture / "truth.json"),
        "qrels_sha256": sha256_file(fixture / "qrels.json"),
        "queries": len(query_ids), "scorer": "Engram adjudicated source-unit qrels @10",
        "engine_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPOSITORY, text=True).strip(),
        "engine_dist": {name: sha256_file(REPOSITORY / f"dist/engine/{name}") for name in (
            "pipeline/search/index.js", "storage/zvec.js", "code-ir/sidecar.js", "extraction/code/ir.js"
        )},
        "adapter_sha256": sha256_file(REPOSITORY / "scripts/qeval_code_ir_v2_search.mjs"),
        "runner_sha256": sha256_file(REPOSITORY / "scripts/replay_code_ir_v2_qeval.py"),
        "ablation": {
            "enabled": args.ablation,
            "implementation_sha256": sha256_file(REPOSITORY / "scripts/code_ir_projection_ablation.mjs") if args.ablation else None,
            "arms": raw["provenance"].get("ablation", {}).get("arms") if args.ablation else None,
        },
        "scorer_sha256": sha256_file(engram_root / "scripts/evaluate_semantic_search.py"),
        "mapping_sha256": sha256_file(engram_root / "scripts/replay_synthetic_semantic_search.py"),
        "model": {"identity": args.model, "device": "cpu", "cache_tree_sha256": tree_digest(model_cache)},
        "runtime": raw["provenance"]["runtime"],
        "code_ir": raw["provenance"]["code_ir"], "scip": None,
        "raw_sha256": sha256_file(raw_path),
        "normalized_sha256": sha256_file(normalized_path),
        "metrics_sha256": sha256_file(metrics_path),
        "comparison_sha256": sha256_file(comparison_path),
    }, indent=2) + "\n")
    return {"output_dir": str(output_dir), "overall": {row["backend"]: row["overall"] for row in metrics["runs"]}, "comparison": comparison}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("fixture", "engram-root", "model-cache", "work-dir", "output-dir"):
        parser.add_argument(f"--{flag}", type=Path, required=True)
    parser.add_argument("--model", default="local/potion-code-16m-v2")
    parser.add_argument(
        "--ablation", action="store_true",
        help="opt-in frozen 2x2 unit-policy/metadata search-view experiment",
    )
    args = parser.parse_args()
    print(json.dumps(run(args), indent=2))


if __name__ == "__main__":
    main()
