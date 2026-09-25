"""Index an immutable Kubernaut commit with the custom scip-go binary.

The live Kubernaut worktree is read-only. `git archive HEAD` stages the committed
revision into a new temporary root, excluding the worktree's untracked files.
This is a producer/source-map coverage probe, not a relevance qeval.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tarfile
from pathlib import Path
from typing import Any


SPIKE_ROOT = Path(__file__).resolve().parents[1]


def git_output(repo: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_set_digest(root: Path, relative_paths: list[str]) -> tuple[str, int]:
    digest = hashlib.sha256()
    total_bytes = 0
    for relative in sorted(relative_paths):
        raw = (root / relative).read_bytes()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(raw)
        digest.update(b"\0")
        total_bytes += len(raw)
    return digest.hexdigest(), total_bytes


def run(args: argparse.Namespace) -> dict[str, Any]:
    repo = args.repo.resolve(strict=True)
    binary = args.scip_go_binary.resolve(strict=True)
    work = args.work_dir.resolve()
    if work.exists():
        raise ValueError("Kubernaut probe work directory must be new")
    work.mkdir(parents=True)
    commit = git_output(repo, "rev-parse", args.revision or "HEAD")
    status = git_output(repo, "status", "--short")
    untracked = [line for line in status.splitlines() if line.startswith("??")]
    modified = [line for line in status.splitlines() if not line.startswith("??")]
    if modified:
        raise ValueError("tracked Kubernaut files are modified; refusing a moving source snapshot")

    staged = work / "kubernaut"
    process = subprocess.Popen(
        ["git", "-C", str(repo), "archive", "--format=tar", commit],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
        archive.extractall(staged, filter="data")
    process.stdout.close()
    stderr = process.stderr.read().decode("utf-8", "replace") if process.stderr else ""
    return_code = process.wait()
    if return_code:
        raise RuntimeError(f"git archive failed ({return_code}): {stderr}")

    index_path = staged / "index.scip"
    command = [
        str(binary),
        "index",
        "./...",
        "--module-root",
        ".",
        "--repository-remote",
        "github.com/jordigilh/kubernaut",
        "--module-version",
        commit,
        "--skip-tests",
        "--output",
        str(index_path),
    ]
    result = subprocess.run(
        command,
        cwd=staged,
        capture_output=True,
        text=True,
        timeout=args.timeout,
        check=False,
        env={**os.environ, "GOTOOLCHAIN": args.go_toolchain},
    )
    (work / "scip-go.stdout.log").write_text(result.stdout)
    (work / "scip-go.stderr.log").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(
            f"custom scip-go exited {result.returncode}; see {work}/scip-go.stderr.log"
        )

    schema_result = subprocess.run(
        ["python3", str(SPIKE_ROOT / "scripts/scip_spike.py"), "schema", str(work)],
        cwd=SPIKE_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    sys.path.insert(0, str(work))
    import scip_pb2  # generated from the pinned protocol schema by scip_spike.py

    index = scip_pb2.Index()
    index_bytes = index_path.read_bytes()
    index.ParseFromString(index_bytes)
    relative_paths = sorted(document.relative_path for document in index.documents)
    source_digest, source_bytes = source_set_digest(staged, relative_paths)
    documents = {document.relative_path: document for document in index.documents}
    if len(documents) != len(relative_paths):
        raise ValueError("SCIP index has duplicate document paths")
    selected_encodings = {document.position_encoding for document in documents.values()}
    if selected_encodings != {1}:
        raise ValueError(f"custom index did not declare UTF-8 for every document: {selected_encodings}")

    sys.path.insert(0, str(SPIKE_ROOT))
    from scripts.scip_spike import range_bytes

    occurrences = definitions = references = valid_primary = 0
    definition_enclosing = valid_definition_enclosing = 0
    invalid_primary = invalid_enclosing = 0
    for relative, document in documents.items():
        raw = (staged / relative).read_bytes()
        text = raw.decode("utf-8")
        for occurrence in document.occurrences:
            occurrences += 1
            try:
                start, end = range_bytes(text, occurrence, 1)
                if not (0 <= start < end <= len(raw)):
                    raise ValueError("empty/out-of-bounds primary range")
                raw[start:end].decode("utf-8")
                valid_primary += 1
            except (ValueError, UnicodeDecodeError):
                invalid_primary += 1
                continue
            if occurrence.symbol_roles & 1:
                definitions += 1
                if occurrence.enclosing_range or occurrence.WhichOneof("typed_enclosing_range"):
                    definition_enclosing += 1
                    try:
                        body_start, body_end = range_bytes(text, occurrence, 1, enclosing=True)
                        if body_start <= start < end <= body_end:
                            valid_definition_enclosing += 1
                        else:
                            invalid_enclosing += 1
                    except ValueError:
                        invalid_enclosing += 1
            elif occurrence.symbol:
                references += 1

    if invalid_primary or invalid_enclosing:
        raise ValueError(f"invalid source ranges: primary={invalid_primary} enclosing={invalid_enclosing}")
    summary = {
        "schema": "kubernaut-go-scip-probe-v1",
        "repository": str(repo),
        "revision": commit,
        "live_worktree_untracked_paths_excluded": len(untracked),
        "index_stdout_first_line": result.stdout.splitlines()[0] if result.stdout.splitlines() else None,
        "producer_binary_sha256": sha256_file(binary),
        "producer_index_sha256": hashlib.sha256(index_bytes).hexdigest(),
        "producer_tool_info": {
            "name": index.metadata.tool_info.name,
            "version": index.metadata.tool_info.version,
            "arguments": list(index.metadata.tool_info.arguments),
        },
        "protocol_sha256": schema_result.stdout.strip().split()[-1],
        "selected_documents": len(documents),
        "source_bytes": source_bytes,
        "selected_source_set_sha256": source_digest,
        "declared_encodings": sorted(selected_encodings),
        "occurrences": occurrences,
        "definitions": definitions,
        "references": references,
        "valid_primary_byte_spans": valid_primary,
        "invalid_primary_byte_spans": invalid_primary,
        "definitions_with_enclosing_range": definition_enclosing,
        "valid_definition_enclosing_ranges": valid_definition_enclosing,
        "invalid_definition_enclosing_ranges": invalid_enclosing,
        "index_path": str(index_path),
        "stdout_log": str(work / "scip-go.stdout.log"),
        "stderr_log": str(work / "scip-go.stderr.log"),
    }
    report_path = work / "kubernaut-go-scip-report.json"
    report_path.write_text(json.dumps(summary, indent=2) + "\n")
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--revision", default="HEAD")
    parser.add_argument("--scip-go-binary", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--go-toolchain", default="auto")
    args = parser.parse_args()
    print(json.dumps(run(args), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
