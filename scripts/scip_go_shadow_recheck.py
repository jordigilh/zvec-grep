"""Persist a validated custom scip-go/Code IR shadow join outside the repo."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path
from typing import Any


def recheck(root: Path, code_ir_root: Path, spike_root: Path, index_sha256: str) -> dict[str, Any]:
    root = root.resolve(strict=True)
    code_ir_root = code_ir_root.resolve(strict=True)
    spike_root = spike_root.resolve(strict=True)
    syntax_path = root / "syntax-ir-go.json"
    syntax = json.loads(syntax_path.read_text())
    process = subprocess.run(
        [
            "python3",
            "-B",
            str(code_ir_root / "scripts/code-ir-shadow-recheck.py"),
            "scip-go",
            str(root),
            "go",
            str(spike_root),
            f"sha256:{index_sha256}",
        ],
        cwd=code_ir_root,
        input=json.dumps(syntax),
        capture_output=True,
        text=True,
        check=False,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
    )
    if process.returncode != 0:
        raise RuntimeError(process.stderr.strip() or process.stdout.strip())
    result = json.loads(process.stdout)
    if result["snapshot"]["snapshot_id"] != syntax["snapshot"]["snapshot_id"]:
        raise ValueError("SCIP shadow join changed the syntax snapshot identity")
    if result.get("producer_artifact_sha256") != index_sha256:
        raise ValueError("SCIP shadow recheck used an unexpected index artifact")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--code-ir-root", type=Path, required=True)
    parser.add_argument("--spike-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--scip-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = recheck(args.root, args.code_ir_root, args.spike_root, args.scip_sha256)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({
        "output": str(args.output.resolve()),
        "producer_artifact_sha256": result["producer_artifact_sha256"],
        "strict_symbols": result["counts"].get("strict_symbol_bindings"),
        "strict_references": result["counts"].get("strict_references"),
        "validated_shadow_facts": len(result["snapshot"]["facts"]),
    }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
