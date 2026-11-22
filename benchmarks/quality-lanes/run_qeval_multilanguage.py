#!/usr/bin/env python3
"""Retrieval quality lane for every four-language fixture (go, python, rust,
typescript), using the non-compact ``--preview full`` CLI output.

Why this runner exists
----------------------
``run_qeval.py`` normalizes the compact CLI output by intersecting the emitted
source span with the fixture's manifest source spans.  Manifest spans are not
guaranteed to exist for every language, so this runner instead maps each hit
from its ``symbol: <kind> <name> [scope: <Scope>]`` line to the manifest unit
id directly:

    no scope  -> "<path>::<name>"
    scope     -> "<path>::<Scope>.<name>"

Every mapped id is validated against the manifest's unit set.  A hit whose
symbol does not resolve (class fields, module declarations, ...) is retried
against manifest source spans when the manifest provides them, and otherwise
counted as an unmatched hit and dropped from the ranking.  A hit that emits no
symbol line at all is retried the same way, and a header whose range is not a
numeric source span is recorded as unmatched instead of being ignored.  A
query whose hits all fail to map is therefore an empty ranking with a non-zero
unmatched count, not an abort.  Every case is reported in ``metrics.json`` so
nothing is silently lost.

The CLI's explicit empty-result labels (``No matches.`` and ``No searchable
files.``) are a successful empty ranking for that query, not an execution
error.

The index is built once by a warmup query with ``--refresh wait`` and every
measured query then runs with ``--refresh off`` so all repeats of all queries
see one frozen index.  Repeats must produce identical normalized unit_id
lists and identical unmatched-hit reports; a mismatch aborts the run.

Usage:
  python3 run_qeval_multilanguage.py --fixture <fixture-dir> --binary <zg> \
      --work-root <dir> --output <dir> [--model-cache <dir>] [--repeats 2]
      [--model <reference>]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from evaluate_semantic_search import evaluate  # noqa: E402
from replay_normalization import ReplayError, copy_fixture_sources  # noqa: E402

RESULT_LIMIT = 10
DEFAULT_MODEL = "local/gte-modernbert-base"

_HEADER = re.compile(
    r"^#(?P<rank>\d+)(?: \[[^\]]+\])?(?: matchedBy=\S+)? "
    r"(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$"
)
# A hit header whose range is not a numeric source span (``file``, ``page:N``,
# ``bytes:a-b``) cannot be mapped from the header alone.  It is still a real
# hit, so it must be recorded as unmatched instead of silently dropped.
_HEADER_LIKE = re.compile(r"^#(?P<rank>\d+)\b.*:\S+$")
# The CLI's explicit empty-result labels.  Both are a successful empty
# ranking, not an execution error.
_EMPTY_RESULT_LABELS = frozenset({"No matches.", "No searchable files."})
# Older binaries: "symbol: function Add scope: WorkflowState".
_SYMBOL_LEGACY = re.compile(
    r"^symbol:\s+(?P<kind>\S+)\s+(?P<name>\S+)"
    r"(?:\s+scope:\s*(?P<scope>\S+))?\s*$"
)
# Newer binaries: "symbol: [function] Add scope: WorkflowState".
_SYMBOL_BRACKETED = re.compile(
    r"^symbol:\s+\[(?P<kind>[^\]]+)\]\s+(?P<name>\S+)"
    r"(?:\s+scope:\s*(?P<scope>\S+))?\s*$"
)


def _parse_symbol(line: str) -> re.Match[str] | None:
    return _SYMBOL_BRACKETED.match(line) or _SYMBOL_LEGACY.match(line)


def _manifest_units(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    return list(manifest["units"])


def _span_units(units: list[dict[str, Any]], path: str, start: int, end: int) -> list[str]:
    """Manifest units whose declared span intersects [start, end] on ``path``."""
    matches = [
        unit
        for unit in units
        if unit.get("path") == path
        and isinstance(unit.get("start_line"), int)
        and isinstance(unit.get("end_line"), int)
        and unit["start_line"] <= end
        and unit["end_line"] >= start
    ]
    matches.sort(key=lambda unit: (unit["start_line"], unit["end_line"], unit["unit_id"]))
    return [unit["unit_id"] for unit in matches]


def parse_preview(
    stdout: str, units: list[dict[str, Any]]
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], dict[str, int]]:
    """Map preview-full stdout to ranked manifest unit ids.

    Returns (ranked_results, unmatched_hits, summary).  ``ranked_results`` is
    deduped in first-seen order and keeps the originating backend rank,
    matching the compact-output normalization used by ``run_qeval.py``.
    """
    known = {unit["unit_id"] for unit in units}
    ranked: list[dict[str, Any]] = []
    matched_by_symbol = 0
    matched_by_span = 0
    seen: set[str] = set()
    unmatched: list[dict[str, Any]] = []

    def record(unit_ids: list[str], method: str, rank: int) -> None:
        nonlocal matched_by_symbol, matched_by_span
        for unit_id in unit_ids:
            if unit_id in seen:
                continue
            seen.add(unit_id)
            ranked.append({
                "unit_id": unit_id,
                "backend_rank": rank,
                "resolved_by": method,
            })
            if method == "symbol":
                matched_by_symbol += 1
            else:
                matched_by_span += 1

    def settle_span_only(hit: dict[str, Any]) -> None:
        """Resolve a hit that produced no symbol line via its manifest span."""
        resolved = _span_units(units, hit["path"], hit["start"], hit["end"])
        if resolved:
            record(resolved, "span", hit["rank"])
            return
        unmatched.append({
            "rank": hit["rank"],
            "path": hit["path"],
            "start": hit["start"],
            "end": hit["end"],
            "symbol_line": "",
            "attempted_unit_id": None,
            "reason": "hit emitted no symbol line and no manifest span matched",
        })

    current: dict[str, Any] | None = None
    header_seen_for_current = False
    for line in stdout.splitlines():
        header = _HEADER.match(line)
        if header:
            # A header without a following symbol line is a file-level chunk:
            # resolve it from its span before moving on.
            if header_seen_for_current and current is not None:
                settle_span_only(current)
            current = {
                "rank": int(header["rank"]),
                "path": header["path"],
                "start": int(header["start"]),
                "end": int(header["end"] or header["start"]),
            }
            header_seen_for_current = True
            continue

        header_like = _HEADER_LIKE.match(line)
        if header_like:
            # A header whose range is not numeric (``file``, ``page:N``, ...)
            # cannot be mapped from the header; keep the drop visible and do
            # not let a later symbol line attach to the previous hit.
            if header_seen_for_current and current is not None:
                settle_span_only(current)
            header_seen_for_current = False
            current = None
            unmatched.append({
                "rank": int(header_like["rank"]),
                "path": None,
                "symbol_line": line.strip(),
                "attempted_unit_id": None,
                "reason": "header range is not a source span",
            })
            continue

        symbol = _parse_symbol(line)
        if not symbol or current is None:
            continue

        scope = symbol["scope"]
        guessed = f"{current['path']}::{scope + '.' if scope else ''}{symbol['name']}"
        resolved: list[str] = []
        method = ""
        if guessed in known:
            resolved = [guessed]
            method = "symbol"
        else:
            resolved = _span_units(units, current["path"], current["start"], current["end"])
            method = "span"

        if not resolved:
            unmatched.append({
                "rank": current["rank"],
                "path": current["path"],
                "start": current["start"],
                "end": current["end"],
                "symbol_line": line.strip(),
                "attempted_unit_id": guessed,
                "reason": "symbol and span both failed to resolve",
            })
            header_seen_for_current = False
            continue

        record(resolved, method, current["rank"])
        header_seen_for_current = False

    if header_seen_for_current and current is not None:
        settle_span_only(current)

    if not ranked and not unmatched:
        stripped = stdout.strip()
        if stripped in _EMPTY_RESULT_LABELS:
            return [], unmatched, {
                "resolved_by_symbol": matched_by_symbol,
                "resolved_by_span": matched_by_span,
                "unmatched": len(unmatched),
            }
        raise ReplayError("zvec response contained no resolvable ranked source spans")

    summary = {
        "resolved_by_symbol": matched_by_symbol,
        "resolved_by_span": matched_by_span,
        "unmatched": len(unmatched),
    }
    return ranked, unmatched, summary


def run_queries(
    binary: Path,
    root: Path,
    queries: list[dict[str, Any]],
    refresh: str,
    model_cache: Path,
) -> list[dict[str, Any]]:
    rows = []
    for query in queries:
        command = [
            str(binary),
            "--mode", "direct",
            "--refresh", refresh,
            "--limit", str(RESULT_LIMIT),
            "--preview", "full",
            "--model-cache", str(model_cache),
            "--device", "cpu",
            "--hybrid", query["query"],
        ]
        started = time.monotonic()
        completed = subprocess.run(
            command, cwd=root, capture_output=True, text=True, check=False
        )
        elapsed = time.monotonic() - started
        if completed.returncode:
            raise SystemExit(
                f"zg failed for {query['id']}: {completed.stderr.strip()[:2000]}"
            )
        rows.append({
            "id": query["id"],
            "query": query["query"],
            "elapsed_seconds": round(elapsed, 3),
            "response": {"stdout": completed.stdout, "stderr": completed.stderr},
        })
    return rows


def _git(cwd: Path, *args: str) -> str | None:
    completed = subprocess.run(
        ["git", "-C", str(cwd), *args], capture_output=True, text=True, check=False
    )
    if completed.returncode:
        return None
    return completed.stdout.strip() or None


def resolved_model(root: Path) -> str | None:
    """Reads the model the engine actually chose for the built index."""
    manifest = root / ".zvec-grep" / "manifest.json"
    if not manifest.exists():
        return None
    try:
        payload = json.loads(manifest.read_text())
    except (OSError, ValueError):
        return None
    embeddings = payload.get("embeddings") or []
    model = embeddings[0].get("model", {}) if embeddings else {}
    name = model.get("name")
    provider = model.get("provider")
    if not name:
        return None
    return f"{provider}/{name}" if provider else name


def build_index(binary: Path, root: Path, model_cache: Path, model: str) -> float:
    """Build the workspace index explicitly when the binary supports it.

    Newer binaries require ``zg --index --embedding <model>`` before any query;
    older binaries auto-create the index on the first ``--refresh wait`` query.
    Both paths are supported: a failed explicit build is ignored because the
    warmup query then creates the index itself.
    """
    command = [
        str(binary),
        "--index",
        "--model-cache", str(model_cache),
        "--device", "cpu",
        "--mode", "direct",
    ]
    # "auto" leaves model selection to the engine (dominant source language),
    # which only a binary that profiles the workspace can do.
    if model and model != "auto":
        command.extend(["--embedding", model])
    started = time.monotonic()
    completed = subprocess.run(
        command, cwd=root, capture_output=True, text=True, check=False
    )
    if completed.returncode:
        # Older binary without the --index command: the warmup query builds it.
        return 0.0
    return round(time.monotonic() - started, 3)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--model-cache",
        type=Path,
        default=Path(os.environ.get(
            "ZVEC_GREP_MODEL_CACHE", str(Path.home() / ".engram/zvec-grep/model")
        )),
    )
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    args = parser.parse_args()

    if args.repeats < 1:
        raise SystemExit("--repeats must be >= 1")

    fixture = args.fixture.resolve()
    binary = args.binary.resolve()
    model_cache = args.model_cache.resolve()
    output = args.output
    if output.exists():
        raise SystemExit(f"output directory already exists: {output}")

    manifest = json.loads((fixture / "manifest.json").read_text(encoding="utf-8"))
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    qrels = json.loads((fixture / "qrels.json").read_text(encoding="utf-8"))
    queries = truth["queries"]
    units = _manifest_units(manifest)

    root = args.work_root.resolve()
    copy_fixture_sources(fixture, manifest, root)

    # Build the index once; every measured query and every repeat sees it frozen.
    warmup_started = time.monotonic()
    build_index(binary, root, model_cache, args.model)
    run_queries(binary, root, queries[:1], "wait", model_cache)
    index_wall_seconds = round(time.monotonic() - warmup_started, 3)

    repeats: list[list[dict[str, Any]]] = []
    repeat_unmatched: list[list[dict[str, Any]]] = []
    repeat_summaries: list[dict[str, Any]] = []
    for _ in range(args.repeats):
        rows = run_queries(binary, root, queries, "off", model_cache)
        normalized: list[dict[str, Any]] = []
        unmatched: list[dict[str, Any]] = []
        totals = {"resolved_by_symbol": 0, "resolved_by_span": 0, "unmatched": 0}
        for row in rows:
            ranked, row_unmatched, summary = parse_preview(
                row["response"]["stdout"], units
            )
            normalized.append({
                "id": row["id"],
                "results": ranked,
                "unmatched_hits": row_unmatched,
            })
            unmatched.extend({"query_id": row["id"], **hit} for hit in row_unmatched)
            for key in totals:
                totals[key] += summary[key]
        repeats.append(normalized)
        repeat_unmatched.append(unmatched)
        repeat_summaries.append(totals)

    # Determinism gate: the normalized ranking *and* the unmatched-hit report
    # must be identical across repeats.  Comparing only resolved unit ids
    # would let a repeat resolve a different hit as unmatched while producing
    # the same ranking, silently changing the mapping report.  Abort loudly
    # with the first divergent query otherwise.
    reference = [[r["unit_id"] for r in q["results"]] for q in repeats[0]]
    reference_unmatched = [q["unmatched_hits"] for q in repeats[0]]
    deterministic = True
    for repeat_index, repeat in enumerate(repeats[1:], start=2):
        candidate = [[r["unit_id"] for r in q["results"]] for q in repeat]
        candidate_unmatched = [q["unmatched_hits"] for q in repeat]
        if candidate != reference or candidate_unmatched != reference_unmatched:
            deterministic = False
            for query_index, (left, right) in enumerate(zip(reference, candidate)):
                if left != right:
                    raise SystemExit(
                        "non-deterministic ranking across repeats: "
                        f"repeat 1 {queries[query_index]['id']} -> {left}; "
                        f"repeat {repeat_index} -> {right}"
                    )
            for query_index, (left, right) in enumerate(
                zip(reference_unmatched, candidate_unmatched)
            ):
                if left != right:
                    raise SystemExit(
                        "non-deterministic unmatched hits across repeats: "
                        f"repeat 1 {queries[query_index]['id']} -> {left}; "
                        f"repeat {repeat_index} -> {right}"
                    )
            raise SystemExit("non-deterministic ranking across repeats")

    runs = {
        "schema_version": 1,
        "suite_id": truth["suite_id"],
        "fixture_id": manifest["fixture_id"],
        "source": qrels["source"],
        "result_limit": RESULT_LIMIT,
        "runs": [{"backend": "zvec-cli-hybrid-preview-full", "queries": repeats[0]}],
    }
    metrics = evaluate(qrels, runs, cutoff=RESULT_LIMIT)
    overall = metrics["runs"][0]["overall"]

    with binary.open("rb") as binary_file:
        binary_sha256 = hashlib.file_digest(binary_file, "sha256").hexdigest()
    # The binary does not embed its build commit, so record the checkout that
    # ran the lane.  ``binary_sha256`` identifies the binary; calling this
    # "commit" would misattribute a prebuilt binary to the current HEAD.
    checkout = Path(__file__).resolve().parent
    checkout_commit = _git(checkout, "rev-parse", "HEAD")
    checkout_branch = _git(checkout, "rev-parse", "--abbrev-ref", "HEAD")

    payload = {
        "schema": "quality-lane-multilanguage-metrics-v1",
        "fixture_id": manifest["fixture_id"],
        "language": manifest.get("language"),
        "provenance": {
            "binary": str(binary),
            "binary_sha256": binary_sha256,
            "checkout_commit": checkout_commit,
            "checkout_branch": checkout_branch,
            "model": args.model,
            "resolved_model": resolved_model(root),
            "device": "cpu",
            "result_limit": RESULT_LIMIT,
            "refresh_measured": "off",
        },
        "fixture": {
            "path": str(fixture),
            "snapshot_sha256": qrels["source"].get("snapshot_sha256"),
            "unit_count": len(units),
        },
        "work_root": str(root),
        "index_wall_seconds": index_wall_seconds,
        "repeats": args.repeats,
        "determinism": {
            "identical": deterministic,
            "repeats": args.repeats,
            "normalized_unit_id_lists": reference,
        },
        "mapping": {
            "totals_per_repeat": repeat_summaries,
            "unmatched_hits_per_repeat": repeat_unmatched,
            "units_matched": len({u for query_units in reference for u in query_units}),
        },
        "per_query": [
            {
                "id": query["id"],
                "query": query["query"],
                "results": repeats[0][index]["results"],
                "unmatched_hits": repeats[0][index]["unmatched_hits"],
            }
            for index, query in enumerate(queries)
        ],
        "overall": {
            "ndcg@10": overall["ndcg@10"],
            "mrr@10": overall["mrr@10"],
            "recall@10": overall["recall@10"],
            "precision@10": overall["precision@10"],
        },
        "scorer": metrics,
    }

    output.mkdir(parents=True)
    (output / "metrics.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps({
        "fixture_id": manifest["fixture_id"],
        "binary_sha256": binary_sha256,
        "checkout_commit": checkout_commit,
        "ndcg@10": overall["ndcg@10"],
        "mrr@10": overall["mrr@10"],
        "recall@10": overall["recall@10"],
        "precision@10": overall["precision@10"],
        "units_matched": payload["mapping"]["units_matched"],
        "unmatched_hits": repeat_summaries[0]["unmatched"],
        "deterministic": deterministic,
        "index_wall_seconds": index_wall_seconds,
    }))


if __name__ == "__main__":
    main()
