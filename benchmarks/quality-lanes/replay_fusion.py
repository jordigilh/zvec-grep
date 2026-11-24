#!/usr/bin/env python3
"""Offline replay of the indexed-search fusion stage over engine traces.

Why this exists
---------------
Issue #68 needs fusion candidates evaluated against every pinned lane floor
before any Rust change. The engine's ``--trace`` output already carries, per
hit, the exact fusion inputs: every recall entry (route, rank, raw score) and
the fused score. This module ports ``candidate_fusion_score`` from
``rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs`` to Python
so scoring-rule variants can be replayed offline over collected traces.

Fidelity contract
-----------------

- ``candidate_fusion_score`` and ``FusionWeights`` are line-for-line ports of
  the Rust implementation (constants included). The unit tests port the Rust
  property tests, including exact-value assertions.
- ``collect_traces`` runs one deep hybrid pass (``--limit 200 --trace``); on
  these fixtures the adaptive recall stage already saturates at its initial
  depth (200) with target ``max(limit*5, 50)`` candidates, so the deep pass
  is a superset of the lane pass's candidate set with identical per-route
  ranks. Replaying with ``limit=10`` then reproduces the lane's fused
  ranking; ``replay_ranking``'s output must be validated against the lane's
  ``normalized-runs.json`` before any variant is believed.
- Known divergence: the engine breaks exact fused-score ties by internal
  entity id ascending; the replay breaks them by deep-run output order
  (candidate ids are ``hit-<deep rank>``). Empirically harmless — the
  baseline replay reproduced every pinned lane metric exactly — but exact
  ties could in principle order differently.

Usage:
  python3 replay_fusion.py --fixture <fixture-dir> \
      (--binary <zg> --work-root <dir> | --traces-from <replay-out-dir>) \
      --output <dir> [--model-cache <dir>] [--expected <normalized-runs.json>] \
      [--variants baseline,norm:minmax,norm:zscore,norm:hybrid[:alpha],norm:capped[:ratio]] \
      [--dump-traces]

  --dump-traces persists the collected per-query candidates (with per-route
  raw scores) so later runs can iterate on variants via --traces-from
  without re-running the engine.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_qeval_multilanguage import (  # noqa: E402
    _SYMBOL_BRACKETED,
    _SYMBOL_LEGACY,
    _span_units,
    build_index,
    resolved_model,
)
from replay_normalization import copy_fixture_sources  # noqa: E402

# --- Constants ported from pipeline.rs -------------------------------------

RRF_K = 60.0
DISCORDANT_ROUTE_RRF_WEIGHT = 0.07
ROUTE_RANK_AGREEMENT_FACTOR = 2.5
MAX_CORROBORATION_RATIO = 0.25
FULL_FUSION_TAIL_MULTIPLIER = 2
PREFER_SYMBOL_SUFFIX = ".prefer-symbol"

# --- FusionWeights port ------------------------------------------------------


class FusionWeights:
    """Per-route (lexical, vector) weights; port of the Rust struct."""

    def __init__(
        self, by_route: dict[str, tuple[float, float]],
        fallback: tuple[float, float],
    ) -> None:
        self.by_route = by_route
        self.fallback = fallback

    def for_route(self, route_id: str) -> tuple[float, float]:
        if route_id in self.by_route:
            return self.by_route[route_id]
        if route_id.endswith(PREFER_SYMBOL_SUFFIX):
            base = route_id[: -len(PREFER_SYMBOL_SUFFIX)]
            if base in self.by_route:
                return self.by_route[base]
        return self.fallback


# --- candidate_fusion_score port ---------------------------------------------


def candidate_fusion_score(
    recall: list[dict[str, Any]],
    limit: int,
    weights: FusionWeights,
    *,
    discordant_weight: float = DISCORDANT_ROUTE_RRF_WEIGHT,
    band_multiplier: int = 1,
    agreement_factor: float = ROUTE_RANK_AGREEMENT_FACTOR,
) -> float:
    """Port of ``candidate_fusion_score``.

    ``discordant_weight``, ``band_multiplier`` and ``agreement_factor`` are
    the knobs the spike varies; their defaults reproduce the shipped
    behaviour exactly. ``band_multiplier`` scales the shallow band edge used
    by the same-fusion-band test (``max <= limit * band_multiplier``);
    ``agreement_factor`` scales the closeness test
    (``max <= min * agreement_factor``).
    """
    best_by_mode: dict[str, tuple[float, int, str]] = {}
    for trace in recall:
        rank = trace.get("rank")
        if rank is None:
            continue
        lexical, vector = weights.for_route(trace["route_id"])
        weight = lexical if trace["path"] == "fts" else vector
        score = weight / (RRF_K + float(rank))
        key = trace["path"]
        best = best_by_mode.get(key)
        if best is None:
            best_by_mode[key] = (score, rank, trace["route_id"])
        else:
            better = (
                score > best[0]
                or (score == best[0] and rank < best[1])
            )
            if better:
                best_by_mode[key] = (score, rank, trace["route_id"])

    route_scores = list(best_by_mode.values())
    if not route_scores:
        return 0.0
    primary_score = max(entry[0] for entry in route_scores)

    min_rank = min(entry[1] for entry in route_scores)
    max_rank = max(entry[1] for entry in route_scores)
    deep_cutoff = limit * FULL_FUSION_TAIL_MULTIPLIER
    routes_agree = (
        len(route_scores) > 1
        and (max_rank <= limit * band_multiplier or min_rank > deep_cutoff)
        and float(max_rank) <= float(min_rank) * agreement_factor
    )
    route_corroboration = sum(
        entry[0] for entry in route_scores
    ) - primary_score
    if routes_agree:
        corroboration = min(route_corroboration, primary_score * MAX_CORROBORATION_RATIO)
    else:
        corroboration = discordant_weight * route_corroboration
    return primary_score + corroboration


def replay_ranking(
    candidates: list[dict[str, Any]],
    limit: int,
    weights: FusionWeights,
    scorer: Callable[
        [list[dict[str, Any]], int, FusionWeights], float
    ] = candidate_fusion_score,
) -> list[dict[str, Any]]:
    """Fuses candidates exactly like ``fuse_candidates`` (score desc, id asc)."""
    for candidate in candidates:
        candidate["fused_score"] = scorer(candidate["recall"], limit, weights)
    candidates.sort(key=lambda c: (-c["fused_score"], c["id"]))
    return candidates


# --- Trace collection ---------------------------------------------------------

_HEADER = re.compile(
    r"^#(?P<rank>\d+)(?: \[[^\]]+\])?(?: matchedBy=\S+)? "
    r"(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$"
)
_TRACE_LINE = re.compile(r"^trace: (?P<payload>\{.*\})\s*$")


def parse_trace_hits(stdout: str) -> list[dict[str, Any]]:
    """Splits a ``--trace`` preview-full run into per-hit records.

    Each record carries the header (rank, path, span), the symbol line and
    the parsed engine trace. Hits are returned in output order; one hit is
    one fused candidate (entity) in the engine's pipeline.
    """
    hits: list[dict[str, Any]] = []
    current: dict[str, Any] | None = None
    for line in stdout.splitlines():
        header = _HEADER.match(line)
        if header:
            if current is not None:
                hits.append(current)
            current = {
                "rank": int(header["rank"]),
                "path": header["path"],
                "start": int(header["start"]),
                "end": int(header["end"] or header["start"]),
            }
            continue
        if current is None:
            continue
        symbol = _SYMBOL_BRACKETED.match(line) or _SYMBOL_LEGACY.match(line)
        if symbol:
            current["kind"] = symbol["kind"]
            current["name"] = symbol["name"]
            current["scope"] = symbol["scope"]
            continue
        trace_match = _TRACE_LINE.match(line)
        if trace_match:
            try:
                current["trace"] = json.loads(trace_match["payload"])
            except ValueError:
                pass
            hits.append(current)
            current = None
    if current is not None:
        hits.append(current)
    return hits


def hit_unit_ids(hit: dict[str, Any], units: list[dict[str, Any]]) -> list[str]:
    """Maps one hit to manifest unit ids exactly like ``parse_preview``."""
    known = {unit["unit_id"] for unit in units}
    scope = hit.get("scope")
    name = hit.get("name")
    if name:
        guessed = f"{hit['path']}::{scope + '.' if scope else ''}{name}"
        if guessed in known:
            return [guessed]
    return _span_units(units, hit["path"], hit["start"], hit["end"])


def candidates_from_hits(
    hits: list[dict[str, Any]], units: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    """One replay candidate per traced hit, with its unit mapping.

    The engine fuses per entity and the deep run orders hits by the engine's
    own fused score — computed with the deep limit, which changes the
    corroboration band. The replay must therefore rescore every hit at the
    lane limit and only then select and map, mirroring ``fuse_candidates``
    followed by the runner's first-seen unit dedup.
    """
    candidates = []
    for hit in hits:
        trace = hit.get("trace")
        if trace is None:
            continue
        candidates.append({
            "id": f"hit-{hit['rank']}",
            "unit_ids": hit_unit_ids(hit, units),
            "recall": [
                {
                    "path": entry["path"],
                    "route_id": entry["route_id"],
                    "rank": entry.get("rank"),
                    "score": entry.get("score"),
                }
                for entry in trace.get("recall", [])
            ],
            "engine_fused_score": trace.get("fusion", {}).get("score"),
        })
    return candidates


def top10_units(
    ranked_hits: list[dict[str, Any]], limit: int = 10
) -> list[str]:
    """Maps the top-``limit`` ranked hits to unit ids, first-seen dedup."""
    top: list[str] = []
    seen: set[str] = set()
    for candidate in ranked_hits[:limit]:
        for unit_id in candidate["unit_ids"]:
            if unit_id not in seen:
                seen.add(unit_id)
                top.append(unit_id)
    return top


def collect_traces(
    binary: Path,
    root: Path,
    queries: list[dict[str, Any]],
    model_cache: Path,
    deep_limit: int = 200,
) -> list[dict[str, Any]]:
    rows = []
    for query in queries:
        command = [
            str(binary), "--mode", "direct", "--refresh", "off",
            "--limit", str(deep_limit), "--preview", "full", "--trace",
            "--model-cache", str(model_cache), "--device", "cpu",
            "--hybrid", query["query"],
        ]
        completed = subprocess.run(
            command, cwd=root, capture_output=True, text=True, check=False
        )
        if completed.returncode:
            raise SystemExit(
                f"zg failed for {query['id']}: {completed.stderr.strip()[:2000]}"
            )
        rows.append({
            "id": query["id"],
            "query": query["query"],
            "stdout": completed.stdout,
        })
    return rows


# --- Trace persistence --------------------------------------------------------


def dump_traces(
    path: Path,
    rows: list[dict[str, Any]],
    provenance: dict[str, Any],
) -> None:
    """Persist parsed per-query candidates (raw scores included).

    Normalization variants calibrate on the candidate set, so iterating on
    them must not re-run the engine: one collection pass dumps the traces,
    later replay passes load them via ``--traces-from``.
    """
    path.write_text(
        json.dumps(
            {"schema": "quality-lane-fusion-traces-v1",
             "provenance": provenance, "rows": rows},
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def load_traces_payload(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema") != "quality-lane-fusion-traces-v1":
        raise SystemExit(f"unsupported traces schema in {path}")
    return payload


def load_traces(path: Path) -> list[dict[str, Any]]:
    return load_traces_payload(path)["rows"]


# --- Scoring variants ---------------------------------------------------------


# --- Per-route score normalization (issue #70) -------------------------------


def best_route_entry(recall: list[dict[str, Any]]) -> tuple[float, int, str] | None:
    """The best-ranked entry across all recall paths (test helper)."""
    entries = route_entries(recall)
    if not entries:
        return None
    return min(
        entries.values(),
        key=lambda entry: (entry[1] is None, entry[1]),
    )


def route_entries(
    recall: list[dict[str, Any]],
) -> dict[str, tuple[float, int | None, str]]:
    """Per recall path, the (raw score, rank, route_id) entry to fuse.

    The entry with the best rank wins per path — the route's own ordering
    is its quality ground truth, and the two routes carry opposite score
    polarity: fts holds a BM25 score (higher is better) while the vector
    route holds the fp16 index' cosine *distance* (lower is better; see
    ``Fragments::search_vector`` and ``route_score_stats``, which detects
    the polarity from the rank/score covariance). Entries without a raw
    score are ignored.
    """
    best: dict[str, tuple[float, int | None, str]] = {}
    for trace in recall:
        score = trace.get("score")
        if score is None:
            continue
        path = trace["path"]
        rank = trace.get("rank")
        entry = (float(score), rank, trace["route_id"])
        current = best.get(path)
        if current is None or (
            entry[1] is not None
            and (current[1] is None or entry[1] < current[1])
        ):
            best[path] = entry
    return best


def route_score_stats(
    candidates: list[dict[str, Any]],
) -> dict[str, dict[str, Any]]:
    """Per-path raw-score statistics over a candidate set, polarity included.

    Normalization is per query: the calibration set is every fused
    candidate's per-path entry. ``higher_is_better`` is derived from the
    rank/score covariance over that set — a score that rises with rank
    rewards worse hits (the vector route's cosine distance), so it is
    inverted before any min-max or z-score normalization.
    """
    entries: dict[str, list[tuple[float, int | None]]] = {}
    for candidate in candidates:
        for path, (score, rank, _route_id) in route_entries(
            candidate["recall"]
        ).items():
            entries.setdefault(path, []).append((score, rank))
    stats: dict[str, dict[str, Any]] = {}
    for path, pairs in entries.items():
        values = [score for score, _rank in pairs]
        count = len(values)
        mean = sum(values) / count
        variance = sum((value - mean) ** 2 for value in values) / count
        ranked = [(score, rank) for score, rank in pairs if rank is not None]
        higher_is_better = True
        if len(ranked) >= 2:
            # Center on the ranked subset: mixing in rank-less scores
            # would shift the covariance's score mean and could flip the
            # detected polarity.
            ranked_mean = sum(score for score, _r in ranked) / len(ranked)
            rank_mean = sum(rank for _s, rank in ranked) / len(ranked)
            covariance = sum(
                (rank - rank_mean) * (score - ranked_mean)
                for score, rank in ranked
            )
            # A score that rises with rank rewards worse hits: flip it.
            higher_is_better = covariance <= 0.0
        stats[path] = {
            "min": min(values),
            "max": max(values),
            "mean": mean,
            "std": variance**0.5,
            "higher_is_better": higher_is_better,
        }
    return stats


def _oriented(score: float, stat: dict[str, Any]) -> float:
    """Raw score flipped to higher-is-better when the route is a distance."""
    return -score if not stat["higher_is_better"] else score


def _minmax(score: float, stat: dict[str, Any]) -> float:
    span = stat["max"] - stat["min"]
    if span <= 0.0:
        return 0.0
    # Base is the oriented minimum of the range: ``min`` for a
    # higher-is-better score, ``max`` for a distance (lower is better).
    base = min(_oriented(stat["min"], stat), _oriented(stat["max"], stat))
    return (_oriented(score, stat) - base) / span


def _zscore_squash(score: float, stat: dict[str, Any]) -> float:
    if stat["std"] <= 0.0:
        return 0.5
    z = (_oriented(score, stat) - _oriented(stat["mean"], stat)) / stat["std"]
    return 0.5 + 0.5 * z / (1.0 + abs(z))


def _rank_term(rank: int | None) -> float:
    """Reciprocal-rank contribution rescaled to (0, 1]."""
    if rank is None:
        return 0.0
    return (RRF_K + 1.0) / (RRF_K + float(rank))


def normalized_fusion_score(
    recall: list[dict[str, Any]],
    limit: int,
    weights: FusionWeights,
    stats: dict[str, dict[str, Any]],
    *,
    mode: str,
    alpha: float = 0.5,
    cap_ratio: float = MAX_CORROBORATION_RATIO,
) -> float:
    """Fuse calibrated per-route scores instead of rank positions.

    ``norm:minmax`` sums the per-route weight times the route's min-max
    normalized raw score (an absent route contributes zero; a distance-type
    score — the vector route's cosine distance — is inverted first, see
    ``route_score_stats``). ``norm:zscore`` replaces the normalization with a squashed z-score in (0, 1) centered on
    the route mean. ``norm:hybrid`` mixes the min-max raw score with the
    reciprocal-rank term rescaled to (0, 1] (``alpha`` = raw share), so a
    candidate present deep on a route still outranks one absent from it.
    ``norm:capped`` scores like ``norm:minmax`` but takes the best per-route
    weighted contribution as primary and adds the other routes as
    corroboration capped at ``cap_ratio`` times the primary — the shipped
    fusion's head protection without its rank-band tests. No variant
    applies a rank-band test.
    """
    del limit  # rank bands are exactly what this family removes
    contributions: dict[str, float] = {}
    for path, (score, rank, route_id) in route_entries(recall).items():
        stat = stats.get(path)
        if stat is None:
            continue
        lexical, vector = weights.for_route(route_id)
        weight = lexical if path == "fts" else vector
        if mode in ("minmax", "capped"):
            normalized = _minmax(score, stat)
        elif mode == "zscore":
            normalized = _zscore_squash(score, stat)
        elif mode == "hybrid":
            normalized = alpha * _minmax(
                score, stat
            ) + (1.0 - alpha) * _rank_term(rank)
        else:  # pragma: no cover - guarded by make_query_variant
            raise ValueError(f"unknown normalization mode: {mode}")
        contributions[path] = weight * normalized
    if mode == "capped":
        if not contributions:
            return 0.0
        primary = max(contributions.values())
        corroboration = sum(contributions.values()) - primary
        return primary + min(corroboration, cap_ratio * primary)
    return sum(contributions.values())


def make_query_variant(
    name: str,
    stats: dict[str, dict[str, float]] | None,
) -> Callable[[list[dict[str, Any]], int, FusionWeights], float]:
    """Build a per-query scorer; normalization variants need ``stats``.

    ``stats`` is ``route_score_stats`` over the query's candidate set. The
    rank-band variants of the previous spike stay per-candidate and ignore
    it; the normalization family refuses to be built without it.
    """
    if name.startswith("norm:"):
        if stats is None:
            raise ValueError(
                f"variant {name} needs per-query route_score_stats"
            )
        spec = name[len("norm:"):]
        alpha = 0.5
        cap_ratio = MAX_CORROBORATION_RATIO
        if ":" in spec:
            spec, parameter = spec.split(":", 1)
            if spec not in ("hybrid", "capped"):
                raise SystemExit(
                    f"unknown variant: {name} "
                    f"(the parameter applies to hybrid/capped only)"
                )
            try:
                value = float(parameter)
            except ValueError:
                raise SystemExit(f"invalid parameter in variant: {name}") from None
            if spec == "hybrid":
                alpha = value
            else:
                cap_ratio = value
        if spec not in ("minmax", "zscore", "hybrid", "capped"):
            raise SystemExit(f"unknown variant: {name}")

        def normalized(
            recall: list[dict[str, Any]], limit: int, w: FusionWeights
        ) -> float:
            return normalized_fusion_score(
                recall, limit, w, stats, mode=spec, alpha=alpha,
                cap_ratio=cap_ratio,
            )

        return normalized
    # Rank-band variants of the previous spike stay per-candidate.
    return make_variant(name)


def make_variant(name: str) -> Callable[
    [list[dict[str, Any]], int, FusionWeights], float
]:
    """Named scoring-rule variants explored by the spike (see the report)."""
    if name == "baseline":
        return candidate_fusion_score
    if name == "band2":
        # Shallow-or-mid agreement: the corroborating band edge moves from
        # ``limit`` to ``2*limit`` (the deep cutoff), so a shallow lexical hit
        # (rank <= limit) corroborates a mid vector hit (rank <= 2*limit).
        def band2(recall: list[dict[str, Any]], limit: int, w: FusionWeights) -> float:
            return candidate_fusion_score(
                recall, limit, w, band_multiplier=FULL_FUSION_TAIL_MULTIPLIER
            )
        return band2
    if name.startswith("discordant"):
        factor = float(name[len("discordant:"):])

        def discordant(recall: list[dict[str, Any]], limit: int, w: FusionWeights) -> float:
            return candidate_fusion_score(recall, limit, w, discordant_weight=factor)
        return discordant
    if name.startswith("agreement"):
        factor = float(name[len("agreement:"):])

        def agreement(recall: list[dict[str, Any]], limit: int, w: FusionWeights) -> float:
            return candidate_fusion_score(recall, limit, w, agreement_factor=factor)
        return agreement
    raise SystemExit(f"unknown variant: {name}")


def weights_for_query(query: str) -> FusionWeights:
    """Weights for one hybrid query, mirroring FusionWeights::for_routes.

    The engine resolves the hybrid plan route into ``fts`` and ``vector``
    routes that share the query, so both route ids carry the query-shape
    weights; synthesized ``.prefer-symbol`` routes inherit them.
    """
    from trace_route_diff import classify

    shapes = {
        "identifier": (1.1, 1.0),
        "mixed": (1.0, 1.0),
        "natural_language": (1.0, 2.0),
    }
    shape = shapes[classify(query)]
    return FusionWeights(
        by_route={"fts": shape, "vector": shape},
        fallback=(1.1, 1.0),
    )


def validate_args(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    """Enforce the --traces-from / collection argument combinations."""
    if args.traces_from is not None:
        if args.binary is not None or args.work_root is not None:
            parser.error(
                "--traces-from cannot be combined with --binary/--work-root"
            )
        if args.dump_traces:
            parser.error("--dump-traces requires collecting, not --traces-from")
    elif args.binary is None or args.work_root is None:
        parser.error("--binary and --work-root are required unless --traces-from")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument(
        "--binary", type=Path, default=None,
        help="zg binary; required unless --traces-from replays dumped traces",
    )
    parser.add_argument(
        "--work-root", type=Path, default=None,
        help="fresh work root for the index; required unless --traces-from",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--model-cache",
        type=Path,
        default=Path(os.environ.get(
            "ZVEC_GREP_MODEL_CACHE", str(Path.home() / ".engram/zvec-grep/model")
        )),
    )
    parser.add_argument("--model", default="auto")
    parser.add_argument("--deep-limit", type=int, default=200)
    parser.add_argument("--variants", default="baseline")
    parser.add_argument("--expected", type=Path, default=None)
    parser.add_argument(
        "--traces-from", type=Path, default=None,
        help="replay traces dumped by a previous run instead of running zg",
    )
    parser.add_argument(
        "--dump-traces", action="store_true",
        help="dump the collected per-query candidates (raw scores) for "
             "later --traces-from replay",
    )
    args = parser.parse_args()
    validate_args(parser, args)

    fixture = args.fixture.resolve()
    output = args.output
    if output.exists():
        raise SystemExit(f"output directory already exists: {output}")

    manifest = json.loads((fixture / "manifest.json").read_text(encoding="utf-8"))
    truth = json.loads((fixture / "truth.json").read_text(encoding="utf-8"))
    qrels = json.loads((fixture / "qrels.json").read_text(encoding="utf-8"))
    units = manifest["units"]
    queries = truth["queries"]

    expected: dict[str, list[str]] | None = None
    if args.expected is not None:
        lane = json.loads(args.expected.read_text(encoding="utf-8"))
        expected = {
            q["id"]: [r["unit_id"] for r in q["results"]]
            for q in lane["runs"][0]["queries"]
        }

    if args.traces_from is not None:
        traces_path = args.traces_from / "traces.json"
        traces_payload = load_traces_payload(traces_path)
        provenance = dict(traces_payload.get("provenance", {}))
        if provenance.get("fixture_id") != manifest["fixture_id"]:
            raise SystemExit(
                f"traces in {traces_path} were collected on fixture "
                f"{provenance.get('fixture_id')!r}, not "
                f"{manifest['fixture_id']!r}"
            )
        rows = traces_payload["rows"]
        expected_query_ids = [q["id"] for q in queries]
        if [row["id"] for row in rows] != expected_query_ids:
            raise SystemExit(
                f"traces in {traces_path} do not cover this fixture's queries"
            )
        provenance["replayed_from"] = str(traces_path)
    else:
        binary = args.binary.resolve()
        root = args.work_root.resolve()
        copy_fixture_sources(fixture, manifest, root)
        build_index(binary, root, args.model_cache, args.model)
        warmup = [
            str(binary), "--mode", "direct", "--refresh", "wait",
            "--limit", "10", "--preview", "full",
            "--model-cache", str(args.model_cache), "--device", "cpu",
            "--hybrid", queries[0]["query"],
        ]
        subprocess.run(
            warmup, cwd=root, capture_output=True, text=True, check=False
        )
        with binary.open("rb") as binary_file:
            binary_sha256 = hashlib.file_digest(
                binary_file, "sha256"
            ).hexdigest()
        collected = collect_traces(
            binary, root, queries, args.model_cache, args.deep_limit
        )
        provenance = {
            "fixture_id": manifest["fixture_id"],
            "binary_sha256": binary_sha256,
            "resolved_model": resolved_model(root),
            "deep_limit": args.deep_limit,
        }
        rows = [{
            "id": row["id"],
            "query": row["query"],
            "candidates": candidates_from_hits(
                parse_trace_hits(row["stdout"]), units
            ),
        } for row in collected]
        if args.dump_traces:
            output.mkdir(parents=True, exist_ok=True)
            dump_traces(output / "traces.json", rows, provenance)

    payload: dict[str, Any] = {
        "schema": "quality-lane-fusion-replay-v1",
        "fixture_id": manifest["fixture_id"],
        "provenance": provenance,
        "variants": {},
    }
    for name in args.variants.split(","):
        per_query = []
        baseline_matches: list[bool] = []
        for row in rows:
            candidates = row["candidates"]
            weights = weights_for_query(row["query"])
            stats = route_score_stats(candidates)
            scorer = make_query_variant(name, stats)
            ranked = replay_ranking(candidates, 10, weights, scorer)
            top10 = top10_units(ranked, 10)
            if expected is not None:
                baseline_matches.append(top10 == expected.get(row["id"]))
            per_query.append({
                "id": row["id"],
                "top10": top10,
                "scores": {
                    unit: candidate["fused_score"]
                    for candidate in ranked[:20]
                    for unit in candidate["unit_ids"]
                },
            })
        runs = {
            "schema_version": 1,
            "suite_id": truth["suite_id"],
            "fixture_id": manifest["fixture_id"],
            "source": qrels["source"],
            "result_limit": 10,
            "runs": [{
                "backend": f"fusion-replay-{name}",
                "queries": [
                    {"id": q["id"], "results": [
                        {"unit_id": u} for u in q["top10"]
                    ]}
                    for q in per_query
                ],
            }],
        }
        from evaluate_semantic_search import evaluate

        metrics = evaluate(qrels, runs, cutoff=10)["runs"][0]["overall"]
        payload["variants"][name] = {
            "per_query": per_query,
            "overall": metrics,
        }
        print(json.dumps({
            "fixture_id": manifest["fixture_id"],
            "variant": name,
            "ndcg@10": metrics["ndcg@10"],
            "mrr@10": metrics["mrr@10"],
            "recall@10": metrics["recall@10"],
            "precision@10": metrics["precision@10"],
            "top10_matches_expected": (
                all(baseline_matches) if expected is not None else None
            ),
        }))

    output.mkdir(parents=True, exist_ok=True)
    (output / "fusion-replay.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
