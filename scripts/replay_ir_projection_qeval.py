#!/usr/bin/env python3
"""Paired same-engine qeval for syntax-only, Code IR projection and SCIP arms.

The source fixture/qrels are read-only. All indexes, projections and run artifacts
are written to a fresh work directory outside the repository.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any


REPOSITORY = Path(__file__).resolve().parents[1]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def tree_digest(root: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    count = 0
    for path in sorted(item for item in root.rglob("*") if item.is_file() and not item.is_symlink()):
        relative = path.relative_to(root).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(sha256_file(path)))
        digest.update(b"\0")
        count += 1
    return digest.hexdigest(), count


def git_revision(root: Path) -> str | None:
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def load_engram_api(engram_root: Path):
    sys.path.insert(0, str(engram_root.resolve()))
    from scripts.build_synthetic_qrels import _snapshot, build_qrels
    from scripts.evaluate_semantic_search import evaluate
    from scripts.replay_synthetic_semantic_search import (
        _dedupe_results,
        _map_chunk,
        _unit_spans,
    )

    return _snapshot, build_qrels, evaluate, _dedupe_results, _map_chunk, _unit_spans


def normalize_arm(
    arm: dict[str, Any],
    units: list[Any],
    map_chunk,
    dedupe_results,
) -> dict[str, Any]:
    queries = []
    for row in arm["queries"]:
        ranked = []
        for hit in row["hits"]:
            unit_ids = map_chunk(
                units,
                hit["path"],
                int(hit["start_line"]),
                int(hit["end_line"]),
            )
            ranked.append((int(hit["rank"]), unit_ids))
        if not ranked:
            raise ValueError(f"{arm['name']} returned no ranked hits for {row['id']}")
        queries.append({"id": row["id"], "results": dedupe_results(ranked)})
    return {"backend": arm["name"], "queries": queries}


def compare_metrics(metrics: dict[str, Any], language: str = "go") -> dict[str, Any]:
    runs = metrics["runs"]
    baseline = runs[0]
    keys = ("ndcg@10", "mrr@10", "recall@10", "precision@10")
    comparisons = []
    for candidate in runs[1:]:
        deltas = {
            key: candidate["overall"][key] - baseline["overall"][key]
            for key in keys
        }
        comparisons.append({
            "baseline": baseline["backend"],
            "candidate": candidate["backend"],
            "aggregate_deltas": deltas,
            "no_regression_gate": all(value >= -1e-12 for value in deltas.values()),
            "improvement_gate": any(value > 1e-12 for value in deltas.values()),
            "per_query_deltas": [
                {
                    "id": baseline_row["id"],
                    **{
                        key: candidate_row[key] - baseline_row[key]
                        for key in keys
                    },
                }
                for baseline_row, candidate_row in zip(
                    baseline["per_query"], candidate["per_query"], strict=True
                )
            ],
        })
    return {"same_engine": True, "same_language": language, "comparisons": comparisons}


def copy_fixture_sources(fixture: Path, manifest: dict[str, Any], roots: list[Path]) -> tuple[list[str], int]:
    paths = sorted({unit["path"] for unit in manifest["units"]})
    total_bytes = 0
    for relative in paths:
        source = fixture / relative
        raw = source.read_bytes()
        total_bytes += len(raw)
        for root in roots:
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(raw)
    return paths, total_bytes


def run(args: argparse.Namespace) -> dict[str, Any]:
    fixture = args.fixture.resolve(strict=True)
    engram_root = args.engram_root.resolve(strict=True)
    runtime_root = args.runtime_root.resolve(strict=True)
    model_cache = args.model_cache.resolve(strict=True)
    work_dir = args.work_dir.resolve()
    output_dir = args.output_dir.resolve()
    if work_dir.exists() or output_dir.exists():
        raise ValueError("work and output directories must be new and empty")
    work_dir.mkdir(parents=True)
    output_dir.mkdir(parents=True)

    (
        snapshot,
        build_qrels,
        evaluate,
        dedupe_results,
        map_chunk,
        unit_spans,
    ) = load_engram_api(engram_root)
    manifest = json.loads((fixture / "manifest.json").read_text())
    truth = json.loads((fixture / "truth.json").read_text())
    qrels = json.loads((fixture / "qrels.json").read_text())
    language = manifest.get("language")
    if language not in {"go", "python", "rust", "typescript"}:
        raise ValueError(f"unsupported frozen qeval lane: {language!r}")
    if manifest.get("fixture_id") != f"{language}-workflow-discovery-v1":
        raise ValueError("fixture ID and source language do not match")
    if args.scip_shadow and language != "go":
        raise ValueError("the current custom scip-go enrichment arm is Go-only")
    if build_qrels(fixture) != qrels:
        raise ValueError("frozen source, manifest, truth and qrels disagree")
    source_digest, selected, source_bytes = snapshot(fixture, manifest)
    if (source_digest, len(selected), source_bytes) != (
        qrels["source"]["snapshot_sha256"],
        qrels["source"]["files"],
        qrels["source"]["bytes"],
    ):
        raise ValueError("frozen source snapshot differs from qrels")

    baseline_root = work_dir / "baseline"
    candidate_root = work_dir / "candidate"
    baseline_root.mkdir()
    candidate_root.mkdir()
    paths, total_bytes = copy_fixture_sources(
        fixture, manifest, [baseline_root, candidate_root]
    )
    if len(paths) != qrels["source"]["files"] or total_bytes != qrels["source"]["bytes"]:
        raise ValueError("paired qeval staging does not match the frozen source inventory")

    inventory_path = None
    if language == "go":
        inventory_path = output_dir / "go-source-unit-inventory-v1.json"
        subprocess.run(
            [
                "go", "run", str(REPOSITORY / "scripts/go_ir_source_inventory.go"),
                "--fixture", str(fixture), "--output", str(inventory_path),
            ],
            check=True,
            cwd=REPOSITORY,
        )
        inventory = json.loads(inventory_path.read_text())
        if inventory["source_set_sha256"] != source_digest or len(inventory["units"]) != 38:
            raise ValueError("independent Go AST inventory is not aligned to qrels")

    raw_path = output_dir / "raw-evidence.json"
    sidecar_root = work_dir / "ir-sidecar"
    index_root = work_dir / "projection-indexes"
    command = [
        "node",
        str(REPOSITORY / "scripts/qeval_ir_projection_search.mjs"),
        "--runtime-root", str(runtime_root),
        "--fixture", str(fixture),
        "--baseline-root", str(baseline_root),
        "--candidate-root", str(candidate_root),
        "--sidecar-root", str(sidecar_root),
        "--index-root", str(index_root),
        "--model-cache", str(model_cache),
        "--model", args.model,
        "--output", str(raw_path),
    ]
    scip_provenance = None
    if args.scip_shadow:
        scip_shadow = args.scip_shadow.resolve(strict=True)
        scip_index = args.scip_index.resolve(strict=True)
        scip_binary = args.scip_binary.resolve(strict=True)
        command.extend([
            "--scip-shadow", str(scip_shadow),
            "--scip-index", str(scip_index),
            "--scip-binary", str(scip_binary),
            "--expected-scip-facts", str(args.expected_scip_facts),
        ])
        scip_provenance = {
            "binary": str(scip_binary),
            "binary_sha256": sha256_file(scip_binary),
            "index": str(scip_index),
            "index_sha256": sha256_file(scip_index),
            "shadow": str(scip_shadow),
            "shadow_sha256": sha256_file(scip_shadow),
        }
    subprocess.run(command, check=True, cwd=REPOSITORY)
    raw = json.loads(raw_path.read_text())

    unit_map = unit_spans(fixture, manifest)
    normalized = {
        "schema_version": 1,
        "suite_id": truth["suite_id"],
        "fixture_id": manifest["fixture_id"],
        "source": {**qrels["source"], "fixture": manifest["fixture_id"]},
        "result_limit": 10,
        "runs": [normalize_arm(arm, unit_map, map_chunk, dedupe_results) for arm in raw["arms"]],
    }
    metrics = evaluate(qrels, normalized, cutoff=10)
    comparison = compare_metrics(metrics, language)

    active = json.loads((sidecar_root / "active.json").read_text())
    snapshot_file = sidecar_root / "generations" / active["ir_snapshot_id"] / "snapshot.json"
    conformance = None
    if inventory_path:
        conformance = json.loads(subprocess.run(
            [
                "python3", str(REPOSITORY / "scripts/compare_go_ir_inventory.py"),
                "--fixture", str(fixture),
                "--inventory", str(inventory_path),
                "--ir", str(snapshot_file),
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout)

    run_manifest = {
        "schema": "code-ir-qeval-run-manifest-v1",
        "fixture_id": manifest["fixture_id"],
        "language": language,
        "source_set_sha256": source_digest,
        "source_files": len(paths),
        "source_bytes": total_bytes,
        "manifest_sha256": sha256_file(fixture / "manifest.json"),
        "truth_sha256": sha256_file(fixture / "truth.json"),
        "qrels_sha256": sha256_file(fixture / "qrels.json"),
        "query_count": len(truth["queries"]),
        "result_limit": 10,
        "scorer": "Engram source-unit qrels nDCG/MRR/Recall/Precision@10",
        "engine_runtime_root": str(runtime_root),
        "engine_revision": git_revision(runtime_root),
        "runtime": raw["provenance"]["runtime"],
        "engine_dist": {
            "search_sha256": sha256_file(runtime_root / "dist/engine/pipeline/search/index.js"),
            "storage_sha256": sha256_file(runtime_root / "dist/engine/storage/zvec.js"),
            "code_ir_sha256": sha256_file(runtime_root / "dist/engine/extraction/code/ir.js"),
            "sidecar_sha256": sha256_file(runtime_root / "dist/engine/code-ir/sidecar.js"),
        },
        "model": {
            "identity": args.model,
            "device": "cpu",
            "cache_root": str(model_cache),
            "cache_tree_sha256": tree_digest(model_cache)[0],
            "cache_files": tree_digest(model_cache)[1],
        },
        "code_ir": raw["provenance"]["code_ir"],
        "go_ast_inventory_sha256": sha256_file(inventory_path) if inventory_path else None,
        "go_ast_exact_unit_matches": conformance["exact_span_kind_and_token_matches"] if conformance else None,
        "go_ast_inventory_units": conformance["inventory_units"] if conformance else None,
        "scip": scip_provenance,
        "raw_sha256": sha256_file(raw_path),
        "normalized_sha256": None,
        "metrics_sha256": None,
    }

    (output_dir / "raw-runs.json").write_text(json.dumps(raw, indent=2) + "\n")
    normalized_path = output_dir / "normalized-runs.json"
    normalized_path.write_text(json.dumps(normalized, indent=2) + "\n")
    metrics_path = output_dir / "metrics-k10.json"
    metrics_path.write_text(json.dumps(metrics, indent=2) + "\n")
    comparison_path = output_dir / "comparison.json"
    comparison_path.write_text(json.dumps(comparison, indent=2) + "\n")
    if conformance:
        (output_dir / "go-ir-conformance.json").write_text(json.dumps(conformance, indent=2) + "\n")
    run_manifest["normalized_sha256"] = sha256_file(normalized_path)
    run_manifest["metrics_sha256"] = sha256_file(metrics_path)
    (output_dir / "run-manifest.json").write_text(json.dumps(run_manifest, indent=2) + "\n")
    return {
        "output_dir": str(output_dir),
        "metrics": metrics["runs"],
        "comparison": comparison,
        "go_ir_conformance": {
            "exact": conformance["counts"]["exact"],
            "units": conformance["inventory_units"],
        } if conformance else None,
        "scip_facts": (raw["provenance"].get("scip") or {}).get("strict_reference_facts", 0),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--engram-root", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--model-cache", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--model", default="local/potion-code-16m-v2")
    parser.add_argument("--scip-shadow", type=Path)
    parser.add_argument("--scip-index", type=Path)
    parser.add_argument("--scip-binary", type=Path)
    parser.add_argument("--expected-scip-facts", type=int, default=117)
    args = parser.parse_args()
    if bool(args.scip_shadow) != bool(args.scip_index) or bool(args.scip_shadow) != bool(args.scip_binary):
        parser.error("--scip-shadow, --scip-index and --scip-binary must be supplied together")
    result = run(args)
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
