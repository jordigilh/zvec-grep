#!/usr/bin/env python3
"""Route trace-diff for one fixture: where does hybrid lose relevant units?

For every query in a fixture's ``truth.json`` this tool runs the zg CLI three
times against one frozen index:

- ``--hybrid --limit 10 --trace`` — the lane's measured configuration, so the
  per-query top-10 is directly comparable with the lane runner's output;
- ``--fts --limit 20`` and ``--vector --limit 20`` — the single-route rankings
  used to locate each missing relevant unit in each leg.

Hits are normalized to manifest unit ids with ``parse_preview`` from
``run_qeval_multilanguage.py`` (verified to reproduce the compact-output
normalization of ``run_qeval.py`` byte-for-byte on ``go-inventory-audit-v1``).
The hybrid pass additionally records, per hit, the CLI's ``matchedBy=`` label
and the engine's per-route recall trace (route ranks and raw scores plus the
fusion score), which is the exact input the fusion stage scored.

Output: a JSON payload and a markdown report, written under ``--output``
(outside the hash-pinned lane tree).

Query shape
-----------

``classify`` is a Python port of ``QueryShape::classify`` from
``rust/crates/zg-engine/src/pipelines/indexed_search/shape.rs``
(sha256 e3adac7196bad9bbd8c27916ede11fd89597942b16b62ee22d16374c01d59a3f at
the time of the port; re-verify on drift). It replicates the Rust rules
exactly, including the CJK single-token rule and the Unicode-aware character
classes; the unit tests mirror the Rust test cases. Only ``classify`` is
ported — fusion weights are irrelevant to tracing.

Usage:
  python3 trace_route_diff.py --fixture <fixture-dir> --binary <zg> \
      --work-root <dir> --output <dir> [--model-cache <dir>] [--model auto] \
      [--route-limit 20]
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
from run_qeval_multilanguage import (  # noqa: E402
    build_index,
    parse_preview,
    resolved_model,
)
from replay_normalization import copy_fixture_sources  # noqa: E402

HYBRID_LIMIT = 10
DEFAULT_ROUTE_LIMIT = 20

_HEADER = re.compile(
    r"^#(?P<rank>\d+)(?: \[[^\]]+\])?(?: matchedBy=(?P<matched>\S+))? "
    r"(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$"
)
_TRACE_LINE = re.compile(r"^trace: (?P<payload>\{.*\})\s*$")

# ---------------------------------------------------------------------------
# Query-shape port (see module docstring for provenance).
# ---------------------------------------------------------------------------

_NATURAL_LANGUAGE_MIN_TOKENS = 4

_STOPWORDS = frozenset("""
a an the this that these those is are was were be been being am do does did
of in on at to for from by with without into onto about as and or but not no
where when how why what which who whom whose their them they our your my his
her its it we you i he she own can cannot could should would will shall may
might must all any some if then else than so such up out over under there
""".split())

_TRIM_CHARS = ".,;:?!()\"'`"


def _has_non_ascii_letter(token: str) -> bool:
    return any(not ch.isascii() and ch.isalpha() for ch in token)


def _has_internal_separator(token: str, separator: str) -> bool:
    for index, rune in enumerate(token):
        if (
            rune == separator
            and index > 0
            and index + 1 < len(token)
            and token[index - 1].isalnum()
            and token[index + 1].isalnum()
        ):
            return True
    return False


def _has_case_boundary(token: str) -> bool:
    for index in range(1, len(token)):
        previous = token[index - 1]
        current = token[index]
        if current.isupper() and previous.islower():
            return True
        if (
            index + 1 < len(token)
            and previous.isupper()
            and current.isupper()
            and token[index + 1].islower()
        ):
            return True
    return False


def _is_identifier_shaped(token: str) -> bool:
    if "_" in token or "::" in token:
        return True
    return (
        _has_internal_separator(token, "/")
        or _has_internal_separator(token, ".")
        or _has_case_boundary(token)
    )


def _is_stopword(token: str) -> bool:
    word = token.strip(_TRIM_CHARS).lower()
    return word in _STOPWORDS


def classify(query: str) -> str:
    """Port of QueryShape::classify; returns identifier|mixed|natural_language."""
    tokens = query.split()
    if len(tokens) <= 1:
        if tokens and _has_non_ascii_letter(tokens[0]):
            return "natural_language"
        return "identifier"

    identifiers = sum(1 for token in tokens if _is_identifier_shaped(token))
    natural = len(tokens) - identifiers
    if identifiers > 0 and natural > 0:
        return "mixed"
    if identifiers > 0:
        return "identifier"
    if (
        "?" in query
        or any(_is_stopword(token) for token in tokens)
        or len(tokens) >= _NATURAL_LANGUAGE_MIN_TOKENS
    ):
        return "natural_language"
    return "mixed"


# ---------------------------------------------------------------------------
# Output scanners (the pinned runner files are never modified).
# ---------------------------------------------------------------------------

def scan_matched_by(stdout: str) -> dict[int, str]:
    """Maps backend rank -> matchedBy label from preview-full headers."""
    matched: dict[int, str] = {}
    for line in stdout.splitlines():
        header = _HEADER.match(line)
        if header and header["matched"]:
            matched[int(header["rank"])] = header["matched"]
    return matched


def parse_hit_traces(stdout: str) -> dict[int, dict[str, Any]]:
    """Maps backend rank -> parsed engine trace JSON from a --trace run.

    A ``trace:`` line belongs to the most recent hit header; the fusion trace
    is the exact per-route input the fusion stage scored for that hit.
    """
    traces: dict[int, dict[str, Any]] = {}
    current_rank: int | None = None
    for line in stdout.splitlines():
        header = _HEADER.match(line)
        if header:
            current_rank = int(header["rank"])
            continue
        trace_match = _TRACE_LINE.match(line)
        if trace_match and current_rank is not None:
            try:
                traces[current_rank] = json.loads(trace_match["payload"])
            except ValueError:
                continue
    return traces


# ---------------------------------------------------------------------------
# Analysis.
# ---------------------------------------------------------------------------

def analyze_query(
    query_id: str,
    query_text: str,
    hybrid: list[str],
    fts: list[str],
    vector: list[str],
    grades: dict[str, int],
    recall_grade_threshold: int = 2,
) -> dict[str, Any]:
    """Missing-relevant analysis for one query.

    ``hybrid``/``fts``/``vector`` are ranked unit-id lists. Recall-relevant
    means ``grade >= recall_grade_threshold``: the vendored scorer counts
    recall@10 over grade >= 2 only (verified against the pinned holdout
    metrics), so grade-1 positives are excluded from the missing-unit
    analysis entirely.
    """
    recall_relevant = {u for u, g in grades.items() if g >= recall_grade_threshold}
    missing: list[dict[str, Any]] = []
    for unit in sorted(recall_relevant - set(hybrid)):
        def rank_of(route: list[str]) -> int | None:
            return route.index(unit) + 1 if unit in route else None
        missing.append({
            "unit_id": unit,
            "grade": grades[unit],
            "lexical_rank": rank_of(fts),
            "vector_rank": rank_of(vector),
        })
    return {
        "id": query_id,
        "query": query_text,
        "shape": classify(query_text),
        "recall_relevant_count": len(recall_relevant),
        "recall_relevant_found": len(recall_relevant & set(hybrid)),
        "missing_relevant": missing,
    }


def render_markdown(header: dict[str, Any], queries: list[dict[str, Any]]) -> str:
    lines = [
        f"# Route trace diff — {header['fixture_id']}",
        "",
        f"- resolved model: `{header.get('resolved_model')}`",
        f"- binary sha256: `{header.get('binary_sha256')}`",
        f"- hybrid limit: {HYBRID_LIMIT}, route limit: {header.get('route_limit')}",
        "",
    ]
    for query in queries:
        lines.append(f"## {query['id']}")
        lines.append("")
        lines.append(f"> {query['query']}")
        lines.append("")
        lines.append(
            f"shape: **{query['shape']}** — recall-relevant found "
            f"{query['recall_relevant_found']}/{query['recall_relevant_count']}"
        )
        if query["missing_relevant"]:
            lines.append("")
            lines.append("| unit | grade | lexical rank | vector rank |")
            lines.append("|---|---:|---:|---:|")
            for miss in query["missing_relevant"]:
                lines.append(
                    f"| `{miss['unit_id']}` | {miss['grade']} | "
                    f"{miss['lexical_rank'] if miss['lexical_rank'] else '—'} | "
                    f"{miss['vector_rank'] if miss['vector_rank'] else '—'} |"
                )
        lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Route runner.
# ---------------------------------------------------------------------------

def run_route(
    binary: Path,
    root: Path,
    route: str,
    query: str,
    limit: int,
    model_cache: Path,
    trace: bool = False,
) -> str:
    command = [
        str(binary),
        "--mode", "direct",
        "--refresh", "off",
        "--limit", str(limit),
        "--preview", "full",
        "--model-cache", str(model_cache),
        "--device", "cpu",
        f"--{route}", query,
    ]
    if trace:
        command.insert(1, "--trace")
    completed = subprocess.run(
        command, cwd=root, capture_output=True, text=True, check=False
    )
    if completed.returncode:
        raise SystemExit(
            f"zg failed for route {route}: {completed.stderr.strip()[:2000]}"
        )
    return completed.stdout


def _ranked_units(stdout: str, units: list[dict[str, Any]]) -> list[str]:
    ranked, _, _ = parse_preview(stdout, units)
    return [hit["unit_id"] for hit in ranked]


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
    parser.add_argument("--model", default="auto")
    parser.add_argument("--route-limit", type=int, default=DEFAULT_ROUTE_LIMIT)
    args = parser.parse_args()

    fixture = args.fixture.resolve()
    binary = args.binary.resolve()
    model_cache = args.model_cache.resolve()
    output = args.output
    if output.exists():
        raise SystemExit(f"output directory already exists: {output}")

    manifest = json.loads((fixture / "manifest.json").read_text(encoding="utf-8"))
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    qrels = json.loads((fixture / "qrels.json").read_text(encoding="utf-8"))
    units = manifest["units"]
    queries = truth["queries"]

    root = args.work_root.resolve()
    copy_fixture_sources(fixture, manifest, root)

    warmup_started = time.monotonic()
    build_index(binary, root, model_cache, args.model)
    first = run_route(
        binary, root, "hybrid", queries[0]["query"], HYBRID_LIMIT, model_cache
    )
    # One wait warmup on the first query, then freeze: same contract as the
    # lane runners. (The warmup pass above ran with refresh off only after the
    # explicit index build; run one wait pass to guarantee a settled index.)
    command = [
        str(binary), "--mode", "direct", "--refresh", "wait",
        "--limit", str(HYBRID_LIMIT), "--preview", "full",
        "--model-cache", str(model_cache), "--device", "cpu",
        "--hybrid", queries[0]["query"],
    ]
    subprocess.run(command, cwd=root, capture_output=True, text=True, check=False)
    indexed_model = resolved_model(root)
    if indexed_model is None:
        raise SystemExit(f"index manifest is missing after the warmup query: {root}")
    index_wall_seconds = round(time.monotonic() - warmup_started, 3)

    grades_by_query = {
        q["id"]: {j["unit_id"]: j["grade"] for j in q["judgments"]}
        for q in qrels["queries"]
    }

    report: list[dict[str, Any]] = []
    for query in queries:
        hybrid_raw = run_route(
            binary, root, "hybrid", query["query"], HYBRID_LIMIT,
            model_cache, trace=True,
        )
        fts_raw = run_route(
            binary, root, "fts", query["query"], args.route_limit, model_cache
        )
        vec_raw = run_route(
            binary, root, "vector", query["query"], args.route_limit, model_cache
        )
        hybrid_units = _ranked_units(hybrid_raw, units)
        analysis = analyze_query(
            query_id=query["id"],
            query_text=query["query"],
            hybrid=hybrid_units,
            fts=_ranked_units(fts_raw, units),
            vector=_ranked_units(vec_raw, units),
            grades=grades_by_query[query["id"]],
        )
        analysis["hybrid_top10"] = [
            {
                "rank": index + 1,
                "unit_id": unit_id,
                "grade": grades_by_query[query["id"]].get(unit_id, 0),
                "matched_by": scan_matched_by(hybrid_raw).get(index + 1),
                "trace": parse_hit_traces(hybrid_raw).get(index + 1),
            }
            for index, unit_id in enumerate(hybrid_units)
        ]
        report.append(analysis)

    with binary.open("rb") as binary_file:
        binary_sha256 = hashlib.file_digest(binary_file, "sha256").hexdigest()

    payload = {
        "schema": "quality-lane-route-trace-v1",
        "fixture_id": manifest["fixture_id"],
        "provenance": {
            "binary": str(binary),
            "binary_sha256": binary_sha256,
            "resolved_model": indexed_model,
            "hybrid_limit": HYBRID_LIMIT,
            "route_limit": args.route_limit,
            "index_wall_seconds": index_wall_seconds,
        },
        "queries": report,
    }
    output.mkdir(parents=True)
    (output / "route-trace.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    (output / "route-trace.md").write_text(
        render_markdown(
            {
                "fixture_id": manifest["fixture_id"],
                "resolved_model": indexed_model,
                "binary_sha256": binary_sha256,
                "route_limit": args.route_limit,
            },
            report,
        ),
        encoding="utf-8",
    )
    total_missing = sum(len(q["missing_relevant"]) for q in report)
    print(json.dumps({
        "fixture_id": manifest["fixture_id"],
        "resolved_model": indexed_model,
        "queries": len(report),
        "queries_complete_recall": sum(
            1 for q in report if not q["missing_relevant"]
        ),
        "missing_relevant_units": total_missing,
        "binary_sha256": binary_sha256,
    }))


if __name__ == "__main__":
    main()
