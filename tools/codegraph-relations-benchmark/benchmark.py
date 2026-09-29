#!/usr/bin/env python3
"""Compare checked-in relation fixtures with Graphify locally.

This is an opt-in comparator, not a CI test.  The fixture truth remains the
oracle; Graphify is run only on copied source inputs and never receives
truth.json.  The scored boundary is deliberately narrow: both tools emit
file/declaration endpoints for imports_from, re_exports, inherits, and
implements.  Calls, references, tests, embedding, and container/import
granularity are reported separately instead of being silently mixed into the
relation metrics.
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
from collections import Counter, defaultdict
from pathlib import Path
from statistics import median
from typing import Any


GRAPHIFY_VERSION = "0.9.71"
FIXTURE_ROOT = Path("rust/crates/zg-codegraph/tests/fixtures/codegraph-relations-20260928")
LANGUAGES = ("go", "rust", "typescript", "python")

# Keep this set explicit.  Adding a relation to the score requires an
# intentional endpoint/granularity decision and a fixture oracle review.
COMPARABLE_RELATIONS = {
    "imports_from": "imports_from",
    "re_exports": "re_exports",
    "inherits": "inherits",
    "implements": "implements",
}
FILE_EXTENSIONS = {".go", ".rs", ".ts", ".tsx", ".py"}
CSV_FIELDS = [
    "language",
    "repeat",
    "tool",
    "relation",
    "status",
    "wall_ms",
    "expected",
    "predicted",
    "tp",
    "fp",
    "fn",
    "precision",
    "recall",
    "unmapped",
]


def run(command: list[str], cwd: Path) -> str:
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
    return completed.stdout


def timed(command: list[str], cwd: Path) -> float:
    started = time.perf_counter()
    run(command, cwd)
    return (time.perf_counter() - started) * 1000.0


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_truth(fixture: Path) -> dict[str, Any]:
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    if truth.get("schema") != "zvec-grep.codegraph-relations-v2":
        raise RuntimeError(f"unsupported relation truth schema in {fixture / 'truth.json'}")
    expected_files = set(truth["source_files"]) | set(truth.get("context_files", []))
    if set(truth["source_sha256"]) != expected_files:
        raise RuntimeError(f"incomplete source hashes in {fixture / 'truth.json'}")
    for relative, expected in truth["source_sha256"].items():
        actual = sha256(fixture / relative)
        if actual != expected:
            raise RuntimeError(
                f"input digest changed for {fixture / relative}: {actual} != {expected}"
            )
    return truth


def copy_inputs(fixture: Path, destination: Path, truth: dict[str, Any]) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    for relative in truth["source_sha256"]:
        source = fixture / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


def graphify_line(value: Any) -> int | None:
    if not isinstance(value, str):
        return None
    match = re.match(r"L(\d+)", value)
    return int(match.group(1)) if match else None


def node_label(node: dict[str, Any]) -> str:
    path = node.get("path")
    return f"{path}::{node['name']}" if path else node["name"]


def zvec_nodes(graph: dict[str, Any]) -> tuple[dict[str, dict[str, Any]], dict[tuple[str, int], list[dict[str, Any]]]]:
    by_id = {node["id"]: node for node in graph.get("nodes", [])}
    by_location: dict[tuple[str, int], list[dict[str, Any]]] = defaultdict(list)
    for node in graph.get("nodes", []):
        path = node.get("path")
        start_line = (node.get("range") or {}).get("start_line")
        if path and isinstance(start_line, int):
            by_location[(path, start_line)].append(node)
    return by_id, by_location


def truth_pairs(
    truth: dict[str, Any], graph: dict[str, Any], relation: str
) -> set[tuple[str, str]]:
    by_name: dict[str, list[dict[str, Any]]] = defaultdict(list)
    labels = set()
    for node in graph.get("nodes", []):
        by_name[node["name"]].append(node)
        labels.add(node_label(node))

    def unique_node(name: str) -> dict[str, Any]:
        candidates = by_name.get(name, [])
        if len(candidates) != 1:
            raise RuntimeError(f"truth endpoint is not unique in zvec graph: {name}")
        return candidates[0]

    # File-level import/re-export qrels carry the resolved endpoint label in
    # the topology oracle.  Use that independent label rather than deriving
    # the expected pair from zvec's relation edge.
    topology_pairs: dict[tuple[str, str, str], tuple[str, str]] = {}
    for neighbor in truth.get("topology_qrels", {}).get("neighbors", []):
        source_name = neighbor["query"]
        source = unique_node(source_name)
        for outgoing in neighbor.get("outgoing", []):
            if outgoing["kind"] != relation:
                continue
            target_label = outgoing["node"]
            if target_label not in labels:
                raise RuntimeError(f"truth topology endpoint is missing from zvec graph: {target_label}")
            topology_pairs[(source_name, relation, outgoing.get("target_name", ""))] = (
                node_label(source),
                target_label,
            )

    pairs: set[tuple[str, str]] = set()
    for expected in truth["relations"]:
        if expected["kind"] != relation:
            continue
        topology_pair = topology_pairs.get((expected["source"], relation, expected["target"]))
        if topology_pair is not None:
            pairs.add(topology_pair)
            continue
        source = unique_node(expected["source"])
        target = unique_node(expected["target"])
        pairs.add((node_label(source), node_label(target)))
    return pairs


def zvec_pairs(graph: dict[str, Any], relation: str) -> set[tuple[str, str]]:
    by_id, _ = zvec_nodes(graph)
    pairs = set()
    for edge in graph.get("edges", []):
        if edge.get("kind") != relation or not edge.get("resolved"):
            continue
        source = by_id.get(edge.get("source"))
        target = by_id.get(edge.get("target"))
        if source is not None and target is not None:
            pairs.add((node_label(source), node_label(target)))
    return pairs


def graphify_node_label(
    node: dict[str, Any],
    by_location: dict[tuple[str, int], list[dict[str, Any]]],
    zvec_graph: dict[str, Any],
) -> str | None:
    path = node.get("source_file")
    line = graphify_line(node.get("source_location"))
    label = str(node.get("label", ""))
    if not path or line is None:
        return None

    candidates = by_location.get((path, line), [])
    if label == path or Path(label).suffix in FILE_EXTENSIONS:
        candidates = [candidate for candidate in zvec_graph.get("nodes", []) if
                      candidate.get("kind") == "file" and candidate.get("path") == path]
    else:
        normalized = label.removesuffix("()")
        named = [
            candidate
            for candidate in candidates
            if candidate.get("name") == normalized
            or candidate.get("qualified_name") == normalized
            or str(candidate.get("qualified_name", "")).endswith(f".{normalized}")
        ]
        if named:
            candidates = named
    if len(candidates) != 1:
        return None
    return node_label(candidates[0])


def graphify_pairs(
    graph: dict[str, Any], zvec_graph: dict[str, Any], relation: str
) -> tuple[set[tuple[str, str]], int, int]:
    _, by_location = zvec_nodes(zvec_graph)
    nodes = {node["id"]: node for node in graph.get("nodes", [])}
    labels = {
        node_id: graphify_node_label(node, by_location, zvec_graph)
        for node_id, node in nodes.items()
    }
    pairs: set[tuple[str, str]] = set()
    unmapped = 0
    raw_count = 0
    for edge in graph.get("edges", []):
        if edge.get("relation") != relation:
            continue
        raw_count += 1
        source_node = nodes.get(edge.get("source"))
        target_node = nodes.get(edge.get("target"))
        # Graphify emits both module-level and symbol-level import/re-export
        # edges.  The zvec contract scores the module/file edge only.
        if relation in {"imports_from", "re_exports"} and target_node is not None:
            if Path(str(target_node.get("label", ""))).suffix not in FILE_EXTENSIONS:
                continue
        source = labels.get(edge.get("source"))
        target = labels.get(edge.get("target"))
        if source is None or target is None:
            unmapped += 1
            continue
        pairs.add((source, target))
    return pairs, unmapped, raw_count


def score(expected: set[tuple[str, str]], predicted: set[tuple[str, str]]) -> dict[str, Any]:
    tp = len(expected & predicted)
    fp = len(predicted - expected)
    fn = len(expected - predicted)
    return {
        "expected": len(expected),
        "predicted": len(predicted),
        "tp": tp,
        "fp": fp,
        "fn": fn,
        "precision": tp / len(predicted) if predicted else 0.0,
        "recall": tp / len(expected) if expected else 1.0,
    }


def relation_counts(graph: dict[str, Any], field: str) -> Counter[str]:
    return Counter(
        edge.get(field)
        for edge in graph.get("edges", [])
        if isinstance(edge.get(field), str)
    )


def boundary_report(
    truth: dict[str, Any], zvec_graph: dict[str, Any], graphify_graph: dict[str, Any]
) -> list[dict[str, Any]]:
    truth_counts = Counter(edge["kind"] for edge in truth["relations"])
    zvec_counts = relation_counts(zvec_graph, "kind")
    graphify_counts = relation_counts(graphify_graph, "relation")
    kinds = {kind for kind in truth_counts if kind not in COMPARABLE_RELATIONS} | {
        kind for kind in zvec_counts if kind not in COMPARABLE_RELATIONS
    }
    kinds |= {kind for kind in graphify_counts if kind not in COMPARABLE_RELATIONS}
    return [
        {
            "relation": kind,
            "oracle_qrel_count": truth_counts.get(kind, 0),
            "zvec_edge_count": zvec_counts.get(kind, 0),
            "graphify_edge_count": graphify_counts.get(kind, 0),
            "status": "not_scored",
            "reason": (
                "outside the explicit file/declaration overlap boundary; review endpoint "
                "granularity or uncertainty semantics before scoring"
            ),
        }
        for kind in sorted(kinds)
    ]


def run_fixture(
    repo: Path,
    language: str,
    fixture: Path,
    output: Path,
    zg: Path,
    repetitions: int,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    truth = load_truth(fixture)
    rows: list[dict[str, Any]] = []
    boundaries: list[dict[str, Any]] = []
    for repeat in range(repetitions):
        repeat_root = output / language / f"repeat-{repeat:02d}"
        graphify_root = repeat_root / "graphify-input"
        zvec_root = repeat_root / "zvec-input"
        copy_inputs(fixture, graphify_root, truth)
        copy_inputs(fixture, zvec_root, truth)

        graphify_output = repeat_root / "graphify-output"
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
        graphify_graph = json.loads(
            (graphify_output / "graphify-out" / "graph.json").read_text(encoding="utf-8")
        )

        zvec_elapsed = timed([str(zg), "--graph", str(zvec_root)], repo)
        zvec_graph = json.loads(
            (zvec_root / ".zvec-grep/codegraph-v2.json").read_text(encoding="utf-8")
        )
        boundaries.append(
            {
                "language": language,
                "repeat": repeat,
                "relations": boundary_report(truth, zvec_graph, graphify_graph),
            }
        )

        for relation, graphify_relation in COMPARABLE_RELATIONS.items():
            expected = truth_pairs(truth, zvec_graph, relation)
            if not expected:
                continue
            zvec = zvec_pairs(zvec_graph, relation)
            graphify, unmapped, raw_count = graphify_pairs(
                graphify_graph, zvec_graph, graphify_relation
            )
            row = {
                "language": language,
                "repeat": repeat,
                "tool": "zvec",
                "relation": relation,
                "status": "scored",
                "wall_ms": zvec_elapsed,
                "unmapped": 0,
            }
            row.update(score(expected, zvec))
            rows.append(row)
            if graphify:
                row = {
                    "language": language,
                    "repeat": repeat,
                    "tool": "graphify",
                    "relation": relation,
                    "status": "scored",
                    "wall_ms": graphify_elapsed,
                    "unmapped": unmapped,
                }
                row.update(score(expected, graphify))
                rows.append(row)
            else:
                boundaries[-1]["relations"].append(
                    {
                        "relation": relation,
                        "oracle_qrel_count": len(expected),
                        "zvec_edge_count": len(zvec),
                        "graphify_edge_count": raw_count,
                        "unmapped_graphify_edges": unmapped,
                        "status": "not_scored",
                        "reason": (
                            "Graphify emitted no compatible endpoint pairs for this relation; "
                            "the relation is outside the measured overlap for this language"
                        ),
                    }
                )
            if unmapped:
                boundaries[-1]["relations"].append(
                    {
                        "relation": relation,
                        "oracle_qrel_count": len(expected),
                        "zvec_edge_count": len(zvec),
                        "graphify_edge_count": raw_count,
                        "unmapped_graphify_edges": unmapped,
                        "status": "partial_mapping",
                        "reason": "Graphify emitted relation edges whose endpoints could not be source-mapped",
                    }
                )
    return rows, boundaries


def summarize(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[(row["language"], row["tool"], row["relation"])].append(row)
    result = []
    for key in sorted(grouped):
        values = grouped[key]
        first = values[0]
        result.append(
            {
                "language": key[0],
                "tool": key[1],
                "relation": key[2],
                "median_ms": median(value["wall_ms"] for value in values),
                "min_ms": min(value["wall_ms"] for value in values),
                "max_ms": max(value["wall_ms"] for value in values),
                **{
                    field: first[field]
                    for field in CSV_FIELDS
                    if field in first
                    and field not in {"language", "repeat", "tool", "relation", "wall_ms"}
                },
            }
        )
    return result


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


def write_results(
    output: Path,
    metadata: dict[str, Any],
    rows: list[dict[str, Any]],
    boundaries: list[dict[str, Any]],
) -> None:
    output.mkdir(parents=True, exist_ok=True)
    payload = {
        "metadata": metadata,
        "comparable_relations": sorted(COMPARABLE_RELATIONS),
        "runs": rows,
        "summary": summarize(rows),
        "unsupported_or_unscored": boundaries,
    }
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
    parser.add_argument("--language", choices=["all", *LANGUAGES], default="all")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--zg", type=Path, default=None)
    parser.add_argument("--repetitions", type=int, default=1)
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

    languages = LANGUAGES if args.language == "all" else (args.language,)
    rows: list[dict[str, Any]] = []
    boundaries: list[dict[str, Any]] = []
    for language in languages:
        fixture = (repo / FIXTURE_ROOT / language).resolve()
        fixture_rows, fixture_boundaries = run_fixture(
            repo, language, fixture, output, zg, args.repetitions
        )
        rows.extend(fixture_rows)
        boundaries.extend(fixture_boundaries)

    metadata = {
        "graphify_version": GRAPHIFY_VERSION,
        "languages": list(languages),
        "repetitions": args.repetitions,
        "zvec_binary": str(zg),
        "host": platform.platform(),
        "python": version([sys.executable, "--version"]),
        "rustc": version(["rustc", "--version"]),
        "fixtures": {
            language: str((repo / FIXTURE_ROOT / language).resolve()) for language in languages
        },
        "local_only": True,
        "ci_dependency": False,
    }
    write_results(output, metadata, rows, boundaries)
    print(json.dumps(metadata, sort_keys=True))
    print(f"results: {output / 'results.csv'}")
    for summary in summarize(rows):
        print(
            f"{summary['language']} {summary['tool']} {summary['relation']}: "
            f"median={summary['median_ms']:.1f}ms "
            f"precision={summary['precision']:.3f} "
            f"recall={summary['recall']:.3f}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
