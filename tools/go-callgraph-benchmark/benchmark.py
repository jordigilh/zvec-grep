#!/usr/bin/env python3
"""Benchmark Graphify and zvec on a source-pinned Go callgraph fixture.

The runner deliberately measures separate phases:

* Graphify code-only extraction;
* zvec syntax-only graph construction;
* Go call-facts production (using a prebuilt helper) and zvec graph
  construction with the facts sidecar.

It writes JSON and CSV so timing and accuracy can be plotted without parsing
human-oriented command output.  Generated working copies contain only Go
sources and module files; truth.json never enters either tool's input corpus.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


GRAPHIFY_VERSION = "0.9.71"
CSV_FIELDS = [
    "repeat",
    "tool",
    "phase",
    "wall_ms",
    "source_files",
    "source_loc",
    "nodes",
    "edges",
    "call_sites",
    "static_tp",
    "static_fp",
    "static_fn",
    "precision",
    "recall",
    "possible_target_tp",
    "possible_target_total",
    "possible_recall",
    "dynamic_promoted",
    "dynamic_total",
]


def default_zg(repo: Path) -> Path:
    for profile in ("release", "debug"):
        candidate = repo / f"rust/target/{profile}/zg"
        if candidate.is_file():
            return candidate
    return repo / "rust/target/release/zg"


def run(command: list[str], cwd: Path, *, quiet: bool = False) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        print(completed.stdout, file=sys.stderr, end="")
        print(completed.stderr, file=sys.stderr, end="")
        raise RuntimeError(f"command failed ({completed.returncode}): {' '.join(command)}")
    if not quiet:
        last_line = completed.stdout.strip().splitlines()
        if last_line:
            print(last_line[-1])
    return completed.stdout


def timed(command: list[str], cwd: Path, *, quiet: bool = False) -> tuple[float, str]:
    started = time.perf_counter()
    output = run(command, cwd, quiet=quiet)
    return (time.perf_counter() - started) * 1000.0, output


def copy_source_fixture(fixture: Path, destination: Path) -> tuple[int, int]:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    files = 0
    loc = 0
    for path in sorted(fixture.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(fixture)
        if path.suffix != ".go" and path.name not in {"go.mod", "go.sum", "go.work", "go.work.sum"}:
            continue
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
        files += 1
        loc += len(path.read_text(encoding="utf-8").splitlines())
    return files, loc


def normalise_source_path(value: str | None, source_root: Path) -> str | None:
    if not value:
        return None
    path = Path(value)
    if path.is_absolute():
        try:
            return path.resolve().relative_to(source_root.resolve()).as_posix()
        except ValueError:
            return path.as_posix()
    return path.as_posix().removeprefix("./")


def load_truth(fixture: Path) -> dict[str, Any]:
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    for relative, expected in truth["source_sha256"].items():
        actual = hashlib.sha256((fixture / relative).read_bytes()).hexdigest()
        if actual != expected:
            raise RuntimeError(f"source digest changed for {relative}: {actual} != {expected}")
    return truth


def validate_callfacts(root: Path, truth: dict[str, Any]) -> None:
    artifact_path = root / ".zvec-grep/go-callfacts-v2.json"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    expected = {
        (call["path"], call["line"]): call
        for call in truth["static_calls"] + truth["possible_calls"] + truth["dynamic_calls"]
    }
    actual = artifact.get("calls", [])
    if len(actual) != len(expected):
        raise RuntimeError(f"Go call-facts count mismatch: {len(actual)} != {len(expected)}")
    for fact in actual:
        key = (fact["path"], fact["start_line"])
        wanted = expected.get(key)
        if wanted is None:
            raise RuntimeError(f"unexpected Go call fact at {key}")
        for field, actual_field in (
            ("caller", "caller"),
            ("target", "target"),
            ("resolution", "resolution"),
            ("possible_targets", "possible_targets"),
        ):
            if fact[actual_field] != wanted[field]:
                raise RuntimeError(
                    f"Go call fact mismatch at {key} for {field}: "
                    f"{fact[actual_field]!r} != {wanted[field]!r}"
                )


def truth_symbols(truth: dict[str, Any]) -> dict[tuple[str, int], str]:
    return {(entry["path"], int(entry["line"])): entry["id"] for entry in truth["symbols"]}


def graphify_metrics(
    graph_path: Path,
    source_root: Path,
    truth: dict[str, Any],
) -> dict[str, Any]:
    graph = json.loads(graph_path.read_text(encoding="utf-8"))
    symbols = truth_symbols(truth)
    node_symbols: dict[str, str] = {}
    for node in graph["nodes"]:
        location = node.get("source_location", "")
        if not location.startswith("L"):
            continue
        key = (normalise_source_path(node.get("source_file"), source_root), int(location[1:]))
        if key in symbols:
            node_symbols[node["id"]] = symbols[key]

    expected = {(call["caller"], call["target"]) for call in truth["static_calls"]}
    possible_sites = {
        (call["path"], call["line"], call["caller"]): set(call["possible_targets"])
        for call in truth["possible_calls"]
    }
    dynamic_sites = {(call["path"], call["line"], call["caller"]) for call in truth["dynamic_calls"]}
    predicted: set[tuple[str, str]] = set()
    possible_predicted: set[tuple[str, str]] = set()
    dynamic_promoted = 0
    edges = [edge for edge in graph["edges"] if edge.get("relation") == "calls"]
    for edge in edges:
        source = node_symbols.get(edge.get("source"))
        target = node_symbols.get(edge.get("target"))
        if source is None or target is None:
            continue
        if edge.get("target") is not None:
            predicted.add((source, target))
        location = edge.get("source_location", "")
        line = int(location[1:]) if isinstance(location, str) and location.startswith("L") else None
        site = (normalise_source_path(edge.get("source_file"), source_root), line, source)
        if site in possible_sites:
            possible_predicted.add((source, target))
        if site in dynamic_sites:
            dynamic_promoted += 1

    possible_expected = {
        (call["caller"], target)
        for call in truth["possible_calls"]
        for target in call["possible_targets"]
    }
    tp = expected & predicted
    fp = predicted - expected
    fn = expected - predicted
    possible_tp = possible_expected & possible_predicted
    return metrics_dict(
        tool="graphify",
        phase="code-only-extract",
        nodes=len(graph["nodes"]),
        edges=len(graph["edges"]),
        source_files=len(truth["source_files"]),
        source_loc=source_loc(truth, source_root),
        static_tp=len(tp),
        static_fp=len(fp),
        static_fn=len(fn),
        possible_target_tp=len(possible_tp),
        possible_target_total=len(possible_expected),
        possible_recall=len(possible_tp) / len(possible_expected) if possible_expected else 1.0,
        dynamic_promoted=dynamic_promoted,
        dynamic_total=len(dynamic_sites),
        call_sites=len(truth["static_calls"] + truth["possible_calls"] + truth["dynamic_calls"]),
    )


def zvec_metrics(
    graph_path: Path,
    source_root: Path,
    truth: dict[str, Any],
    *,
    tool: str,
    phase: str,
) -> dict[str, Any]:
    graph = json.loads(graph_path.read_text(encoding="utf-8"))
    symbols = truth_symbols(truth)
    node_symbols: dict[str, str] = {}
    node_paths: dict[str, str | None] = {}
    for node in graph["nodes"]:
        node_paths[node["id"]] = normalise_source_path(node.get("path"), source_root)
        source_path = node.get("path")
        location = node.get("range") or {}
        if node.get("kind") not in {"function", "method"} or "start_line" not in location:
            continue
        key = (normalise_source_path(source_path, source_root), int(location["start_line"]))
        if key in symbols:
            node_symbols[node["id"]] = symbols[key]

    expected = {(call["caller"], call["target"]) for call in truth["static_calls"]}
    possible_expected = {
        (call["caller"], target)
        for call in truth["possible_calls"]
        for target in call["possible_targets"]
    }
    possible_sites = {
        (call["path"], call["line"], call["caller"]): set(call["possible_targets"])
        for call in truth["possible_calls"]
    }
    dynamic_sites = {(call["path"], call["line"], call["caller"]) for call in truth["dynamic_calls"]}
    predicted: set[tuple[str, str]] = set()
    possible_predicted: set[tuple[str, str]] = set()
    dynamic_promoted = 0
    edges = [edge for edge in graph["edges"] if edge.get("kind") == "calls"]
    for edge in edges:
        source = node_symbols.get(edge.get("source"))
        if source is None:
            continue
        location = edge.get("range") or {}
        line = location.get("start_line")
        target = node_symbols.get(edge.get("target")) if edge.get("target") else None
        if target is not None and edge.get("resolved"):
            predicted.add((source, target))
        site = (node_paths.get(edge.get("source")), line, source)
        candidates = edge.get("ambiguous_candidates") or []
        for candidate in candidates:
            candidate_symbol = node_symbols.get(candidate)
            if candidate_symbol is not None:
                possible_predicted.add((source, candidate_symbol))
        if target is not None and site in possible_sites:
            possible_predicted.add((source, target))
        if target is not None and site in dynamic_sites:
            dynamic_promoted += 1

    tp = expected & predicted
    fp = predicted - expected
    fn = expected - predicted
    possible_tp = possible_expected & possible_predicted
    return metrics_dict(
        tool=tool,
        phase=phase,
        nodes=len(graph["nodes"]),
        edges=len(graph["edges"]),
        source_files=len(truth["source_files"]),
        source_loc=source_loc(truth, source_root),
        static_tp=len(tp),
        static_fp=len(fp),
        static_fn=len(fn),
        possible_target_tp=len(possible_tp),
        possible_target_total=len(possible_expected),
        possible_recall=len(possible_tp) / len(possible_expected) if possible_expected else 1.0,
        dynamic_promoted=dynamic_promoted,
        dynamic_total=len(dynamic_sites),
        call_sites=len(truth["static_calls"] + truth["possible_calls"] + truth["dynamic_calls"]),
    )


def source_loc(truth: dict[str, Any], source_root: Path) -> int:
    return sum(
        len((source_root / path).read_text(encoding="utf-8").splitlines())
        for path in truth["source_files"]
    )


def metrics_dict(**values: Any) -> dict[str, Any]:
    static_tp = int(values.get("static_tp", 0))
    emitted = static_tp + int(values.get("static_fp", 0))
    values["precision"] = static_tp / emitted if emitted else 0.0
    expected = static_tp + int(values.get("static_fn", 0))
    values["recall"] = static_tp / expected if expected else 0.0
    return values


def write_results(output: Path, metadata: dict[str, Any], rows: list[dict[str, Any]]) -> None:
    payload = {"metadata": metadata, "runs": rows}
    (output / "results.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    with (output / "results.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field, "") for field in CSV_FIELDS})


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--zg", type=Path, default=None)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    if args.repetitions < 1:
        raise SystemExit("--repetitions must be positive")
    fixture = args.fixture.resolve()
    repo = args.repo.resolve()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        if not args.force:
            raise SystemExit(f"refusing non-empty output directory: {output} (use --force)")
        shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)
    truth = load_truth(fixture)
    zg = (args.zg or default_zg(repo)).resolve()
    if not zg.is_file():
        raise SystemExit(f"zvec binary not found: {zg}; build it before benchmarking")

    helper = output / "go-callfacts"
    build_started = time.perf_counter()
    run(["go", "-C", "tools/go-callfacts", "build", "-o", str(helper), "."], repo, quiet=True)
    helper_build_ms = (time.perf_counter() - build_started) * 1000.0

    rows: list[dict[str, Any]] = []
    for repeat in range(args.repetitions):
        repeat_root = output / f"repeat-{repeat:02d}"
        repeat_root.mkdir()
        graphify_input = repeat_root / "graphify-input"
        source_files, source_lines = copy_source_fixture(fixture, graphify_input)
        graphify_output = repeat_root / "graphify-output"
        elapsed, _ = timed(
            [
                "uv",
                "tool",
                "run",
                "--from",
                f"graphifyy=={GRAPHIFY_VERSION}",
                "graphify",
                "extract",
                str(graphify_input),
                "--code-only",
                "--no-cluster",
                "--out",
                str(graphify_output),
            ],
            repo,
            quiet=True,
        )
        graphify_row = graphify_metrics(graphify_output / "graphify-out/graph.json", graphify_input, truth)
        graphify_row.update(repeat=repeat, wall_ms=elapsed, source_files=source_files, source_loc=source_lines)
        rows.append(graphify_row)

        syntax_input = repeat_root / "zvec-syntax"
        copy_source_fixture(fixture, syntax_input)
        syntax_artifact = syntax_input / ".zvec-grep/codegraph-v2.json"
        elapsed, _ = timed(
            [str(zg), "--graph", str(syntax_input), "--output", str(syntax_artifact)],
            repo,
            quiet=True,
        )
        syntax_row = zvec_metrics(
            syntax_input / ".zvec-grep/codegraph-v2.json",
            syntax_input,
            truth,
            tool="zvec",
            phase="syntax-only-graph",
        )
        syntax_row.update(repeat=repeat, wall_ms=elapsed, source_files=source_files, source_loc=source_lines)
        rows.append(syntax_row)

        type_input = repeat_root / "zvec-type-aware"
        copy_source_fixture(fixture, type_input)
        elapsed_facts, _ = timed(
            [str(helper), "--root", str(type_input), "--write-sidecar"],
            repo,
            quiet=True,
        )
        validate_callfacts(type_input, truth)
        facts_row = {
            "repeat": repeat,
            "tool": "zvec-go-callfacts",
            "phase": "produce-sidecar",
            "wall_ms": elapsed_facts,
            "source_files": source_files,
            "source_loc": source_lines,
            "call_sites": len(truth["static_calls"] + truth["possible_calls"] + truth["dynamic_calls"]),
        }
        rows.append(facts_row)
        type_artifact = type_input / ".zvec-grep/codegraph-v2.json"
        elapsed_graph, _ = timed(
            [str(zg), "--graph", str(type_input), "--output", str(type_artifact)],
            repo,
            quiet=True,
        )
        type_row = zvec_metrics(
        type_input / ".zvec-grep/codegraph-v2.json",
            type_input,
            truth,
            tool="zvec",
            phase="go-type-aware-graph",
        )
        type_row.update(repeat=repeat, wall_ms=elapsed_graph, source_files=source_files, source_loc=source_lines)
        rows.append(type_row)
        rows.append(
            {
                "repeat": repeat,
                "tool": "zvec",
                "phase": "go-type-aware-end-to-end",
                "wall_ms": elapsed_facts + elapsed_graph,
                "source_files": source_files,
                "source_loc": source_lines,
                "call_sites": len(truth["static_calls"] + truth["possible_calls"] + truth["dynamic_calls"]),
                **{key: type_row.get(key, "") for key in (
                    "static_tp", "static_fp", "static_fn", "precision", "recall",
                    "possible_target_tp", "possible_target_total", "possible_recall",
                    "dynamic_promoted", "dynamic_total",
                )},
            }
        )

    metadata = {
        "fixture": str(fixture),
        "graphify_version": GRAPHIFY_VERSION,
        "repetitions": args.repetitions,
        "source_files": len(truth["source_files"]),
        "source_loc": source_loc(truth, fixture),
        "static_calls": len(truth["static_calls"]),
        "possible_calls": len(truth["possible_calls"]),
        "dynamic_calls": len(truth["dynamic_calls"]),
        "helper_build_ms": helper_build_ms,
        "zvec_binary": str(zg),
    }
    write_results(output, metadata, rows)
    print(json.dumps(metadata, sort_keys=True))
    print(f"results: {output / 'results.csv'}")


if __name__ == "__main__":
    main()
