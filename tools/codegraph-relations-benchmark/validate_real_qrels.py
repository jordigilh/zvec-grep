#!/usr/bin/env python3
"""Validate manually adjudicated qrels against a real-repository codegraph.

The qrels are intentionally small witness sets, not whole-repository accuracy
claims.  They pin the checkout revision and SHA-256 digests for the source
files that justify each relation.  This validator is opt-in and local-only;
it is not part of the zvec runtime, build, or CI dependencies.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


SCHEMA = "zvec-grep.codegraph-real-qrels-v1"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def node_label(node: dict[str, Any]) -> str:
    path = node.get("path")
    return f"{path}::{node['name']}" if path else node["name"]


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise RuntimeError(f"expected a JSON object in {path}")
    return value


def git_revision(root: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"cannot resolve git revision for {root}: {completed.stderr.strip()}")
    return completed.stdout.strip()


def run_communities(zg: Path, graph: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [str(zg), "--graph-query", str(graph), "communities"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"community query failed ({completed.returncode}): {completed.stderr.strip()}"
        )
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"community query did not return JSON: {error}") from error
    if not isinstance(value, dict):
        raise RuntimeError("community query returned a non-object")
    return value


def validate_source_pins(root: Path, qrels: dict[str, Any]) -> int:
    expected_revision = qrels.get("revision")
    actual_revision = git_revision(root)
    if expected_revision != actual_revision:
        raise RuntimeError(
            f"git revision changed for {root}: {actual_revision} != {expected_revision}"
        )

    source_sha256 = qrels.get("source_sha256")
    if not isinstance(source_sha256, dict) or not source_sha256:
        raise RuntimeError("qrels must contain non-empty source_sha256 witness files")
    for relative, expected in source_sha256.items():
        path = root / relative
        if not path.is_file():
            raise RuntimeError(f"missing pinned witness source: {path}")
        actual = sha256(path)
        if actual != expected:
            raise RuntimeError(f"source digest changed for {relative}: {actual} != {expected}")
    return len(source_sha256)


def relation_key(relation: dict[str, Any]) -> tuple[str, str, str]:
    try:
        return relation["kind"], relation["source"], relation["target"]
    except KeyError as error:
        raise RuntimeError(f"relation qrel is missing {error.args[0]}: {relation}") from error


def validate_relations(graph: dict[str, Any], qrels: dict[str, Any]) -> dict[str, int]:
    nodes = graph.get("nodes")
    edges = graph.get("edges")
    if not isinstance(nodes, list) or not isinstance(edges, list):
        raise RuntimeError("graph must contain nodes and edges arrays")

    labels: dict[str, list[dict[str, Any]]] = {}
    by_id: dict[str, dict[str, Any]] = {}
    for node in nodes:
        label = node_label(node)
        labels.setdefault(label, []).append(node)
        by_id[node["id"]] = node

    def resolve(label: str) -> dict[str, Any]:
        candidates = labels.get(label, [])
        if len(candidates) != 1:
            raise RuntimeError(
                f"qrel endpoint is not unique in graph ({label}): {len(candidates)} candidates"
            )
        return candidates[0]

    def matching_edges(relation: dict[str, Any]) -> list[dict[str, Any]]:
        kind, source_label, target_label = relation_key(relation)
        source = resolve(source_label)
        target = resolve(target_label)
        return [
            edge
            for edge in edges
            if edge.get("kind") == kind
            and edge.get("source") == source["id"]
            and edge.get("target") == target["id"]
            and edge.get("resolved") is True
        ]

    positive = qrels.get("relations", [])
    negative = qrels.get("absent_relations", [])
    if not isinstance(positive, list) or not isinstance(negative, list):
        raise RuntimeError("relations and absent_relations must be arrays")

    categories: dict[str, int] = {}
    for relation in positive:
        if not isinstance(relation, dict):
            raise RuntimeError(f"relation qrel must be an object: {relation}")
        matches = matching_edges(relation)
        if not matches:
            raise RuntimeError(f"missing positive relation qrel: {relation}")
        target_name = relation.get("target_name")
        if target_name is not None and not any(
            edge.get("target_name") == target_name for edge in matches
        ):
            raise RuntimeError(f"relation target_name mismatch: {relation}")
        category = relation.get("category", "structural")
        categories[category] = categories.get(category, 0) + 1

    for relation in negative:
        if not isinstance(relation, dict):
            raise RuntimeError(f"absent relation qrel must be an object: {relation}")
        if matching_edges(relation):
            raise RuntimeError(f"unexpected negative relation qrel: {relation}")

    # Validate every declared endpoint even when a future edge matcher changes.
    for relation in positive + negative:
        resolve(relation["source"])
        resolve(relation["target"])

    return {
        "positive": len(positive),
        "negative": len(negative),
        **{f"category_{key}": value for key, value in sorted(categories.items())},
    }


def validate_communities(communities: dict[str, Any], qrels: dict[str, Any]) -> dict[str, int]:
    assignments = communities.get("assignments")
    if not isinstance(assignments, list):
        raise RuntimeError("community output must contain assignments")
    by_function = {
        assignment["function"]: assignment["community_id"]
        for assignment in assignments
        if isinstance(assignment, dict)
        and isinstance(assignment.get("function"), str)
        and isinstance(assignment.get("community_id"), int)
    }
    pairs = qrels.get("community_pairs", [])
    if not isinstance(pairs, list):
        raise RuntimeError("community_pairs must be an array")
    counts = {"same": 0, "different": 0}
    for pair in pairs:
        if not isinstance(pair, dict):
            raise RuntimeError(f"community qrel must be an object: {pair}")
        left = pair["left"]
        right = pair["right"]
        expected = pair["expected"]
        if left not in by_function or right not in by_function:
            raise RuntimeError(f"community endpoint is absent from assignments: {pair}")
        actual = "same" if by_function[left] == by_function[right] else "different"
        if actual != expected:
            raise RuntimeError(f"community qrel mismatch: {pair}; actual={actual}")
        counts[expected] = counts.get(expected, 0) + 1
    return {
        "pairs": len(pairs),
        "same": counts.get("same", 0),
        "different": counts.get("different", 0),
        "community_count": int(communities.get("community_count", 0)),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True, help="real repository or scoped checkout")
    parser.add_argument("--qrels", type=Path, required=True, help="source-pinned qrel JSON")
    parser.add_argument("--graph", type=Path, required=True, help="plain JSON codegraph artifact")
    parser.add_argument("--zg", type=Path, required=True, help="built zg executable")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    qrels = load_json(args.qrels)
    if qrels.get("schema") != SCHEMA:
        raise RuntimeError(f"unsupported qrel schema in {args.qrels}")
    graph = load_json(args.graph)
    pinned = validate_source_pins(args.root, qrels)
    relation_summary = validate_relations(graph, qrels)
    community_summary = validate_communities(run_communities(args.zg, args.graph), qrels)
    report = {
        "repository": qrels.get("repository"),
        "revision": qrels.get("revision"),
        "scope": qrels.get("scope"),
        "pinned_witness_files": pinned,
        "graph_files": len(graph.get("files", [])),
        "graph_nodes": len(graph.get("nodes", [])),
        "graph_edges": len(graph.get("edges", [])),
        "relations": relation_summary,
        "communities": community_summary,
        "status": "passed",
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
