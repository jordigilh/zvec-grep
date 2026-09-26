#!/usr/bin/env python3
"""Stage a new source-pinned synthetic lane and run opt-in paired search/evidence evaluation.

The old Engram fixtures are not inputs. Work/output must be fresh external paths.
Build with `npm run build` first; pass an existing local CPU model cache.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parents[1]


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(args: argparse.Namespace) -> dict:
    fixture = args.fixture.resolve(strict=True)
    cache = args.model_cache.resolve(strict=True)
    work, output = args.work_dir.resolve(), args.output_dir.resolve()
    if (
        any(path.exists() for path in (work, output))
        or any(path.is_relative_to(REPOSITORY) or path.is_relative_to(fixture)
               for path in (work, output))
        or work == output or work.is_relative_to(output) or output.is_relative_to(work)
    ):
        raise ValueError("work/output must be fresh separate directories outside the checkout")
    truth = json.loads((fixture / "truth.json").read_text())
    if truth["schema_version"] != 1 or truth["fixture_id"] != f"controlled-{truth['language']}-v1":
        raise ValueError("unsupported controlled fixture")
    for relative, digest in truth["files"].items():
        if relative not in (f"core.{dict(go='go', python='py', rust='rs', typescript='ts')[truth['language']]}",
                            f"decoy.{dict(go='go', python='py', rust='rs', typescript='ts')[truth['language']]}"):
            raise ValueError("unexpected source path")
        path = fixture / relative
        if path.is_symlink() or sha(path) != digest:
            raise ValueError(f"stale source: {relative}")
    work.mkdir()
    output.mkdir()
    for root in (work / "baseline", work / "candidate"):
        root.mkdir()
        for relative in truth["files"]:
            (root / relative).write_bytes((fixture / relative).read_bytes())
    raw = output / "raw-runs.json"
    subprocess.run([
        "node", str(REPOSITORY / "scripts/qeval_code_ir_v2_search.mjs"),
        "--controlled", "--fixture", str(fixture),
        "--baseline-root", str(work / "baseline"),
        "--candidate-root", str(work / "candidate"),
        "--sidecar-root", str(work / "ir-sidecar"),
        "--index-root", str(work / "indexes"),
        "--model-cache", str(cache), "--model", args.model,
        "--output", str(raw),
    ], cwd=REPOSITORY, check=True)
    subprocess.run([
        "node", str(REPOSITORY / "scripts/score_code_ir_controlled.mjs"),
        str(fixture), str(work), str(output), args.model,
    ], cwd=REPOSITORY, check=True)
    report = json.loads((output / "strict-results.json").read_text())
    manifest = {
        "schema": "code-ir-controlled-run-v1",
        "fixture_id": truth["fixture_id"],
        "language": truth["language"],
        "source_set_sha256": truth["source_set_sha256"],
        "truth_sha256": sha(fixture / "truth.json"),
        "engine_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPOSITORY, text=True).strip(),
        "model": args.model,
        "model_cache": str(cache),
        "snapshot_id": report["provenance"]["snapshot_id"],
        "implementation_sha256": {name: sha(REPOSITORY / name) for name in (
            "scripts/code_ir_controlled_suite.mjs",
            "scripts/qeval_code_ir_v2_search.mjs",
            "scripts/score_code_ir_controlled.mjs",
            "scripts/replay_code_ir_controlled.py",
            "dist/engine/extraction/code/ir.js",
            "dist/engine/code-ir/sidecar.js",
            "dist/engine/pipeline/search/index.js",
        )},
        "raw_sha256": sha(raw),
        "strict_results_sha256": sha(output / "strict-results.json"),
    }
    (output / "run-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return {"output_dir": str(output), "aggregate": report["aggregate"], "audit": report["audit"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("fixture", "model-cache", "work-dir", "output-dir"):
        parser.add_argument(f"--{flag}", type=Path, required=True)
    parser.add_argument("--model", default="local/potion-code-16m-v2")
    print(json.dumps(run(parser.parse_args()), indent=2))
