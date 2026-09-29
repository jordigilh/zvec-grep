#!/usr/bin/env python3
"""Benchmark language call-facts producers against Graphify on frozen fixtures.

The benchmark keeps the oracle outside every tool input.  It measures separate
phases for Graphify extraction, zvec syntax-only graph construction, the
language-specific call-facts producer, and zvec's semantic graph construction.
Accuracy is limited to the checked-in synthetic fixtures and reports definite
static edges separately from possible and non-definite calls.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import platform
import re
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path
from statistics import median
from typing import Any


GRAPHIFY_VERSION = "0.9.71"
FIXTURE_PATHS = {
    "rust": Path("rust/crates/zg-codegraph/tests/fixtures/rust-graphify-holdout-20260928"),
    "typescript": Path(
        "rust/crates/zg-codegraph/tests/fixtures/typescript-graphify-holdout-20260928"
    ),
    "python": Path("rust/crates/zg-codegraph/tests/fixtures/python-graphify-holdout-20260928"),
}
STATIC_RESOLUTIONS = {"static"}
POSSIBLE_RESOLUTIONS = {"possible", "ambiguous", "trait-dispatch"}
CALLFACTS_SCHEMAS = {
    "rust": "zvec-grep.rust-callfacts",
    "typescript": "zvec-grep.typescript-callfacts",
    "python": "zvec-grep.python-callfacts",
}
CSV_FIELDS = [
    "language",
    "repeat",
    "tool",
    "phase",
    "wall_ms",
    "source_files",
    "source_loc",
    "call_sites",
    "static_tp",
    "static_fp",
    "static_fn",
    "precision",
    "recall",
    "possible_target_tp",
    "possible_target_total",
    "possible_recall",
    "unsafe_definite_sites",
    "class_correct",
    "class_total",
    "class_accuracy",
]


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
        lines = completed.stdout.strip().splitlines()
        if lines:
            print(lines[-1])
    return completed.stdout


def timed(command: list[str], cwd: Path) -> float:
    started = time.perf_counter()
    run(command, cwd, quiet=True)
    return (time.perf_counter() - started) * 1000.0


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_truth(fixture: Path) -> dict[str, Any]:
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    if truth.get("schema") != "zvec-grep.callfacts-benchmark-v1":
        raise RuntimeError(f"unsupported truth schema in {fixture / 'truth.json'}")
    for relative, expected in truth["input_sha256"].items():
        actual = sha256(fixture / relative)
        if actual != expected:
            raise RuntimeError(
                f"input digest changed for {fixture / relative}: {actual} != {expected}"
            )
    if set(truth["input_sha256"]) != set(truth["input_files"]):
        raise RuntimeError(f"truth input digest set is incomplete in {fixture / 'truth.json'}")
    return truth


def copy_inputs(fixture: Path, destination: Path, truth: dict[str, Any]) -> tuple[int, int]:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    for relative in truth["input_files"]:
        source = fixture / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    source_loc = sum(
        len((destination / relative).read_text(encoding="utf-8").splitlines())
        for relative in truth["source_files"]
    )
    return len(truth["source_files"]), source_loc


def line_for_byte(root: Path, path: str, offset: int) -> int:
    data = (root / path).read_bytes()
    if offset < 0 or offset > len(data):
        raise RuntimeError(f"byte offset outside {path}: {offset}")
    return data[:offset].count(b"\n") + 1


def truth_symbols(truth: dict[str, Any]) -> dict[tuple[str, int], str]:
    symbols: dict[tuple[str, int], str] = {}
    for symbol in truth["symbols"]:
        key = (symbol["path"], int(symbol["line"]))
        if key in symbols:
            raise RuntimeError(f"duplicate truth symbol location: {key}")
        symbols[key] = symbol["id"]
    return symbols


def truth_calls(truth: dict[str, Any]) -> dict[tuple[str, int], dict[str, Any]]:
    calls: dict[tuple[str, int], dict[str, Any]] = {}
    for call in truth["calls"]:
        key = (call["path"], int(call["line"]))
        if key in calls:
            raise RuntimeError(f"duplicate truth call location: {key}")
        calls[key] = call
    return calls


def map_zvec_nodes(graph: dict[str, Any], truth: dict[str, Any]) -> dict[str, str]:
    symbols = truth_symbols(truth)
    mapped: dict[str, str] = {}
    for node in graph.get("nodes", []):
        location = node.get("range") or {}
        path = node.get("path")
        line = location.get("start_line")
        if not path or not isinstance(line, int):
            continue
        symbol = symbols.get((path, line))
        if symbol is not None:
            mapped[node["id"]] = symbol
    return mapped


def graphify_line(value: Any) -> int | None:
    if not isinstance(value, str):
        return None
    match = re.match(r"L(\d+)", value)
    return int(match.group(1)) if match else None


def map_graphify_nodes(graph: dict[str, Any], truth: dict[str, Any]) -> dict[str, str]:
    symbols = truth_symbols(truth)
    mapped: dict[str, str] = {}
    for node in graph.get("nodes", []):
        path = node.get("source_file")
        line = graphify_line(node.get("source_location"))
        if not path or line is None:
            continue
        symbol = symbols.get((path, line))
        if symbol is not None:
            mapped[node["id"]] = symbol
    return mapped


def expected_sets(truth: dict[str, Any]) -> tuple[
    set[tuple[str, str]],
    set[tuple[str, str]],
    set[tuple[str, int, str]],
    set[tuple[str, int, str]],
]:
    expected_static: set[tuple[str, str]] = set()
    expected_possible: set[tuple[str, str]] = set()
    possible_sites: set[tuple[str, int, str]] = set()
    nondefinite_sites: set[tuple[str, int, str]] = set()
    for call in truth["calls"]:
        site = (call["path"], int(call["line"]), call["caller"])
        if call["expected_class"] == "definite":
            expected_static.add((call["caller"], call["target"]))
        elif call["expected_class"] == "possible":
            possible_sites.add(site)
            expected_possible.update(
                (call["caller"], target) for target in call.get("possible_targets", [])
            )
        elif call["expected_class"] == "non-definite":
            nondefinite_sites.add(site)
        else:
            raise RuntimeError(f"unknown expected class: {call['expected_class']}")
    return expected_static, expected_possible, possible_sites, nondefinite_sites


def prediction_metrics(
    truth: dict[str, Any],
    predicted: set[tuple[str, str]],
    possible_predicted: set[tuple[str, str]],
    definite_sites: set[tuple[str, int, str]],
) -> dict[str, Any]:
    expected_static, expected_possible, possible_sites, nondefinite_sites = expected_sets(truth)
    static_tp = expected_static & predicted
    static_fp = predicted - expected_static
    static_fn = expected_static - predicted
    possible_tp = expected_possible & possible_predicted
    emitted = len(static_tp) + len(static_fp)
    expected = len(static_tp) + len(static_fn)
    possible_total = len(expected_possible)
    unsafe_sites = definite_sites & (possible_sites | nondefinite_sites)
    return {
        "call_sites": len(truth["calls"]),
        "static_tp": len(static_tp),
        "static_fp": len(static_fp),
        "static_fn": len(static_fn),
        "precision": len(static_tp) / emitted if emitted else 0.0,
        "recall": len(static_tp) / expected if expected else 0.0,
        "possible_target_tp": len(possible_tp),
        "possible_target_total": possible_total,
        "possible_recall": len(possible_tp) / possible_total if possible_total else 1.0,
        "unsafe_definite_sites": len(unsafe_sites),
    }


def zvec_predictions(
    graph: dict[str, Any], root: Path, truth: dict[str, Any]
) -> tuple[set[tuple[str, str]], set[tuple[str, str]], set[tuple[str, int, str]]]:
    node_symbols = map_zvec_nodes(graph, truth)
    call_index = truth_calls(truth)
    possible_sites = {
        (call["path"], int(call["line"]), call["caller"])
        for call in truth["calls"]
        if call["expected_class"] == "possible"
    }
    predicted: set[tuple[str, str]] = set()
    possible_predicted: set[tuple[str, str]] = set()
    definite_sites: set[tuple[str, int, str]] = set()
    for edge in graph.get("edges", []):
        if edge.get("kind") != "calls":
            continue
        source = node_symbols.get(edge.get("source"))
        target = node_symbols.get(edge.get("target")) if edge.get("target") else None
        range_data = edge.get("range") or {}
        line = range_data.get("start_line")
        if source is None or not isinstance(line, int):
            continue
        path = next(
            (
                candidate_path
                for candidate_path, candidate_line in call_index
                if candidate_line == line
                and call_index[(candidate_path, candidate_line)]["caller"] == source
            ),
            None,
        )
        if path is None:
            continue
        site = (path, line, source)
        if target is not None and edge.get("resolved"):
            predicted.add((source, target))
            definite_sites.add(site)
        if target is not None and site in possible_sites:
            possible_predicted.add((source, target))
        if site in possible_sites:
            for candidate in edge.get("ambiguous_candidates") or []:
                candidate_symbol = node_symbols.get(candidate)
                if candidate_symbol is not None:
                    possible_predicted.add((source, candidate_symbol))
    return predicted, possible_predicted, definite_sites


def graphify_predictions(
    graph: dict[str, Any], truth: dict[str, Any]
) -> tuple[set[tuple[str, str]], set[tuple[str, str]], set[tuple[str, int, str]]]:
    node_symbols = map_graphify_nodes(graph, truth)
    possible_sites = {
        (call["path"], int(call["line"]), call["caller"]): call
        for call in truth["calls"]
        if call["expected_class"] == "possible"
    }
    predicted: set[tuple[str, str]] = set()
    possible_predicted: set[tuple[str, str]] = set()
    definite_sites: set[tuple[str, int, str]] = set()
    for edge in graph.get("edges", []):
        if edge.get("relation") != "calls":
            continue
        source = node_symbols.get(edge.get("source"))
        target = node_symbols.get(edge.get("target"))
        line = graphify_line(edge.get("source_location"))
        path = edge.get("source_file")
        if source is None or target is None or line is None or not path:
            continue
        predicted.add((source, target))
        site = (path, line, source)
        definite_sites.add(site)
        if site in possible_sites:
            possible_predicted.add((source, target))
    return predicted, possible_predicted, definite_sites


def fact_symbol_id(root: Path, truth: dict[str, Any], symbol: dict[str, Any] | None) -> str | None:
    if not symbol:
        return None
    line = line_for_byte(root, symbol["path"], int(symbol["start_byte"]))
    return truth_symbols(truth).get((symbol["path"], line))


def fact_metrics(root: Path, truth: dict[str, Any], artifact_path: Path) -> dict[str, Any]:
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    if artifact.get("schema") != CALLFACTS_SCHEMAS[truth["language"]] or artifact.get("version") != 1:
        raise RuntimeError(f"unexpected {truth['language']} call-facts artifact: {artifact_path}")
    calls = truth_calls(truth)
    facts = artifact.get("calls", [])
    facts_by_key: dict[tuple[str, int], dict[str, Any]] = {}
    for fact in facts:
        key = (fact["path"], int(fact["start_line"]))
        if key in facts_by_key:
            raise RuntimeError(f"duplicate producer call location: {key}")
        facts_by_key[key] = fact
    if set(facts_by_key) != set(calls):
        raise RuntimeError(
            f"{truth['language']} producer call locations differ: "
            f"actual={sorted(facts_by_key)} expected={sorted(calls)}"
        )

    predicted: set[tuple[str, str]] = set()
    possible_predicted: set[tuple[str, str]] = set()
    definite_sites: set[tuple[str, int, str]] = set()
    class_correct = 0
    allowed_nondefinite = 0
    for key, call in calls.items():
        fact = facts_by_key[key]
        resolution = fact.get("resolution")
        actual_class = (
            "definite"
            if resolution in STATIC_RESOLUTIONS
            else "possible"
            if resolution in POSSIBLE_RESOLUTIONS
            else "non-definite"
        )
        if actual_class == call["expected_class"]:
            class_correct += 1
        if (
            call["expected_class"] == "non-definite"
            and resolution in set(call.get("allowed_resolutions", []))
        ):
            allowed_nondefinite += 1

        caller = fact_symbol_id(root, truth, fact.get("caller"))
        target = fact_symbol_id(root, truth, fact.get("target"))
        if resolution in STATIC_RESOLUTIONS and caller is not None and target is not None:
            predicted.add((caller, target))
            definite_sites.add((key[0], key[1], caller))
        if caller is not None and call["expected_class"] == "possible":
            for possible_target in fact.get("possible_targets", []):
                target_id = fact_symbol_id(root, truth, possible_target)
                if target_id is not None:
                    possible_predicted.add((caller, target_id))

    metrics = prediction_metrics(truth, predicted, possible_predicted, definite_sites)
    metrics.update(
        class_correct=class_correct,
        class_total=len(calls),
        class_accuracy=class_correct / len(calls) if calls else 0.0,
        allowed_nondefinite=allowed_nondefinite,
    )
    return metrics


def assert_overlay_consumed(graph: dict[str, Any], language: str) -> None:
    key = f"{language}_callfacts_context_sha256"
    if not graph.get(key):
        raise RuntimeError(f"zvec rejected the {language} call-facts sidecar")


def producer_command(repo: Path, language: str, root: Path) -> list[str]:
    if language == "rust":
        return [
            str(repo / "tools/rust-callfacts/generate.sh"),
            "--root",
            str(root),
            "--manifest-path",
            str(root / "Cargo.toml"),
            "--no-all-targets",
        ]
    if language == "typescript":
        return [
            "node",
            str(repo / "tools/typescript-callfacts/generate.mjs"),
            "--root",
            str(root),
            "--project",
            "tsconfig.json",
        ]
    if language == "python":
        return [
            sys.executable,
            str(repo / "tools/python-callfacts/generate.py"),
            "--root",
            str(root),
            "--project",
            "pyrightconfig.json",
        ]
    raise RuntimeError(f"unsupported language: {language}")


def sidecar_path(root: Path, language: str) -> Path:
    names = {
        "rust": "rust-callfacts-v1.json",
        "typescript": "typescript-callfacts-v1.json",
        "python": "python-callfacts-v1.json",
    }
    return root / ".zvec-grep" / names[language]


def graph_row(
    language: str,
    repeat: int,
    tool: str,
    phase: str,
    elapsed: float,
    source_files: int,
    source_loc: int,
    metrics: dict[str, Any],
) -> dict[str, Any]:
    return {
        "language": language,
        "repeat": repeat,
        "tool": tool,
        "phase": phase,
        "wall_ms": elapsed,
        "source_files": source_files,
        "source_loc": source_loc,
        **metrics,
    }


def benchmark_fixture(
    repo: Path,
    language: str,
    fixture: Path,
    output: Path,
    zg: Path,
    repetitions: int,
) -> list[dict[str, Any]]:
    truth = load_truth(fixture)
    language_output = output / language
    language_output.mkdir(parents=True, exist_ok=True)
    rows: list[dict[str, Any]] = []
    for repeat in range(repetitions):
        repeat_root = language_output / f"repeat-{repeat:02d}"
        repeat_root.mkdir()
        source_files = len(truth["source_files"])
        source_loc = sum(
            len((fixture / relative).read_text(encoding="utf-8").splitlines())
            for relative in truth["source_files"]
        )

        graphify_root = repeat_root / "graphify"
        graphify_files, graphify_loc = copy_inputs(fixture, graphify_root, truth)
        graphify_output = graphify_root / "graphify-output"
        graphify_elapsed = timed(
            [
                "uv",
                "tool",
                "run",
                "--from",
                f"graphifyy=={GRAPHIFY_VERSION}",
                "graphify",
                "extract",
                str(graphify_root),
                "--code-only",
                "--no-cluster",
                "--out",
                str(graphify_output),
            ],
            repo,
        )
        graphify_path = graphify_output / "graphify-out" / "graph.json"
        graphify_graph = json.loads(graphify_path.read_text(encoding="utf-8"))
        graphify_predictions_data = graphify_predictions(graphify_graph, truth)
        rows.append(
            graph_row(
                language,
                repeat,
                "graphify",
                "code-only-extract",
                graphify_elapsed,
                graphify_files,
                graphify_loc,
                prediction_metrics(truth, *graphify_predictions_data),
            )
        )

        syntax_root = repeat_root / "zvec-syntax"
        syntax_files, syntax_loc = copy_inputs(fixture, syntax_root, truth)
        syntax_elapsed = timed([str(zg), "--graph", str(syntax_root)], repo)
        syntax_graph = json.loads((syntax_root / ".zvec-grep/codegraph-v2.json").read_text())
        syntax_predictions_data = zvec_predictions(syntax_graph, syntax_root, truth)
        rows.append(
            graph_row(
                language,
                repeat,
                "zvec",
                "syntax-only-graph",
                syntax_elapsed,
                syntax_files,
                syntax_loc,
                prediction_metrics(truth, *syntax_predictions_data),
            )
        )

        semantic_root = repeat_root / "zvec-semantic"
        semantic_files, semantic_loc = copy_inputs(fixture, semantic_root, truth)
        producer_elapsed = timed(producer_command(repo, language, semantic_root), repo)
        facts = fact_metrics(semantic_root, truth, sidecar_path(semantic_root, language))
        rows.append(
            graph_row(
                language,
                repeat,
                f"zvec-{language}-callfacts",
                "produce-sidecar",
                producer_elapsed,
                semantic_files,
                semantic_loc,
                facts,
            )
        )
        semantic_graph_elapsed = timed([str(zg), "--graph", str(semantic_root)], repo)
        semantic_graph = json.loads(
            (semantic_root / ".zvec-grep/codegraph-v2.json").read_text()
        )
        assert_overlay_consumed(semantic_graph, language)
        semantic_predictions_data = zvec_predictions(semantic_graph, semantic_root, truth)
        semantic_metrics = prediction_metrics(truth, *semantic_predictions_data)
        rows.append(
            graph_row(
                language,
                repeat,
                "zvec",
                f"{language}-semantic-graph",
                semantic_graph_elapsed,
                semantic_files,
                semantic_loc,
                semantic_metrics,
            )
        )
        rows.append(
            graph_row(
                language,
                repeat,
                "zvec",
                f"{language}-semantic-end-to-end",
                producer_elapsed + semantic_graph_elapsed,
                semantic_files,
                semantic_loc,
                semantic_metrics,
            )
        )
    return rows


def version(command: list[str]) -> str:
    try:
        completed = subprocess.run(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
    except OSError as error:
        return str(error)
    return completed.stdout.strip().splitlines()[0] if completed.stdout.strip() else "unknown"


def summarize(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[(row["language"], row["tool"], row["phase"])].append(row)
    output: list[dict[str, Any]] = []
    for key in sorted(grouped):
        values = grouped[key]
        first = values[0]
        output.append(
            {
                "language": key[0],
                "tool": key[1],
                "phase": key[2],
                "median_ms": median(row["wall_ms"] for row in values),
                "min_ms": min(row["wall_ms"] for row in values),
                "max_ms": max(row["wall_ms"] for row in values),
                **{
                    field: first.get(field)
                    for field in CSV_FIELDS
                    if field
                    not in {
                        "language",
                        "repeat",
                        "tool",
                        "phase",
                        "wall_ms",
                    }
                },
            }
        )
    return output


def write_results(output: Path, metadata: dict[str, Any], rows: list[dict[str, Any]]) -> None:
    output.mkdir(parents=True, exist_ok=True)
    payload = {"metadata": metadata, "runs": rows, "summary": summarize(rows)}
    (output / "results.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    with (output / "results.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field, "") for field in CSV_FIELDS})


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--language", choices=["all", *FIXTURE_PATHS], default="all")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--zg", type=Path, default=None)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    if args.repetitions < 1:
        raise SystemExit("--repetitions must be positive")

    repo = args.repo.resolve()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        if not args.force:
            raise SystemExit(f"refusing non-empty output directory: {output} (use --force)")
        shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)
    zg = (args.zg or repo / "rust/target/debug/zg").resolve()
    if not zg.is_file():
        raise SystemExit(f"zvec binary not found: {zg}; build it before benchmarking")

    languages = list(FIXTURE_PATHS) if args.language == "all" else [args.language]
    rows: list[dict[str, Any]] = []
    for language in languages:
        fixture = (repo / FIXTURE_PATHS[language]).resolve()
        rows.extend(benchmark_fixture(repo, language, fixture, output, zg, args.repetitions))

    metadata = {
        "graphify_version": GRAPHIFY_VERSION,
        "languages": languages,
        "repetitions": args.repetitions,
        "zvec_binary": str(zg),
        "host": platform.platform(),
        "python": version([sys.executable, "--version"]),
        "node": version(["node", "--version"]),
        "rustc": version(["rustc", "--version"]),
        "rust_callfacts_rustc": version(
            ["rustup", "run", "1.98.0-aarch64-apple-darwin", "rustc", "--version"]
        ),
        "fixtures": {language: str((repo / FIXTURE_PATHS[language]).resolve()) for language in languages},
    }
    write_results(output, metadata, rows)
    print(json.dumps(metadata, sort_keys=True))
    print(f"results: {output / 'results.csv'}")
    for summary in summarize(rows):
        print(
            f"{summary['language']} {summary['tool']} {summary['phase']}: "
            f"median={summary['median_ms']:.1f}ms "
            f"precision={summary.get('precision', '')} "
            f"recall={summary.get('recall', '')}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
