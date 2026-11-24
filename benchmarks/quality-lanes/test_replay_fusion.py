#!/usr/bin/env python3
"""Tests for replay_fusion.py (offline fusion replay over engine traces).

The scoring port must reproduce the Rust ``candidate_fusion_score`` exactly:
these tests port the two property tests from
rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs
(``strong_route_rank_survives_conflicting_weak_route_evidence`` and
``a_second_route_is_corroborating_but_does_not_dominate_the_best_route``),
including their exact-value assertions, plus the tie-break rule from
``equal_weighted_contributions_keep_the_better_rank``.
"""
from __future__ import annotations

import importlib
import pathlib
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
replay_fusion = importlib.import_module("replay_fusion")

IDENTIFIER_WEIGHTS = replay_fusion.FusionWeights(
    by_route={}, fallback=(1.1, 1.0)
)


def recall(path: str, rank: int, route_id: str | None = None) -> dict:
    return {
        "path": path,
        "route_id": route_id or path,
        "rank": rank,
    }


class CandidateFusionScoreTest(unittest.TestCase):
    def test_strong_route_rank_survives_conflicting_weak_route_evidence(self) -> None:
        score = replay_fusion.candidate_fusion_score
        w = IDENTIFIER_WEIGHTS
        # (fts 1, vector 84) beats (fts 10, vector 2)
        self.assertGreater(
            score([recall("fts", 1), recall("vector", 84)], 10, w),
            score([recall("fts", 10), recall("vector", 2)], 10, w),
        )
        # (fts 1, vector 84) beats (fts 14, vector 10)
        self.assertGreater(
            score([recall("fts", 1), recall("vector", 84)], 10, w),
            score([recall("fts", 14), recall("vector", 10)], 10, w),
        )
        # (fts 1, vector 84) beats (fts 26, vector 12)
        self.assertGreater(
            score([recall("fts", 1), recall("vector", 84)], 10, w),
            score([recall("fts", 26), recall("vector", 12)], 10, w),
        )
        # (fts 2, vector 47) beats (fts 30, vector 25)
        self.assertGreater(
            score([recall("fts", 2), recall("vector", 47)], 10, w),
            score([recall("fts", 30), recall("vector", 25)], 10, w),
        )
        # (fts 2, vector 36) beats (fts 3, vector 11)
        self.assertGreater(
            score([recall("fts", 2), recall("vector", 36)], 10, w),
            score([recall("fts", 3), recall("vector", 11)], 10, w),
        )
        # (fts 4, vector 6) beats (fts 5, vector 1)
        self.assertGreater(
            score([recall("fts", 4), recall("vector", 6)], 10, w),
            score([recall("fts", 5), recall("vector", 1)], 10, w),
        )

    def test_second_route_is_corroborating_but_does_not_dominate(self) -> None:
        score = replay_fusion.candidate_fusion_score
        w = IDENTIFIER_WEIGHTS
        self.assertGreater(
            score([recall("fts", 1), recall("vector", 1)], 10, w),
            score([recall("fts", 1)], 10, w),
        )
        self.assertGreater(
            score([recall("fts", 2), recall("vector", 36)], 10, w),
            score([recall("fts", 3), recall("vector", 11)], 10, w),
        )
        # Exact value: deep agreement is capped corroboration.
        value = score([recall("fts", 76), recall("vector", 39)], 10, w)
        vector_rank = 1.0 / (60.0 + 39.0)
        self.assertAlmostEqual(
            value, vector_rank + vector_rank * 0.25, places=12
        )
        # Exact value: transition-band disagreement is discounted corroboration.
        value = score([recall("fts", 26), recall("vector", 12)], 10, w)
        fts_transition = 1.1 / (60.0 + 26.0)
        vector_transition = 1.0 / (60.0 + 12.0)
        self.assertAlmostEqual(
            value, vector_transition + 0.07 * fts_transition, places=12
        )

    def test_equal_weighted_contributions_keep_the_better_rank(self) -> None:
        # fts rank 1 (identifier route) vs two vector traces of equal weighted
        # contribution (identifier rank 1 = 1/61, natural-language rank 62 =
        # 2/122): the rank-1 trace must be retained whichever order the
        # traces are visited in.
        weights = replay_fusion.FusionWeights(
            by_route={
                "identifier": (1.1, 1.0),
                "natural": (1.0, 2.0),
            },
            fallback=(1.1, 1.0),
        )
        lexical = recall("fts", 1, "identifier")
        rank_one = recall("vector", 1, "identifier")
        rank_62 = recall("vector", 62, "natural")
        only_deep = replay_fusion.candidate_fusion_score(
            [lexical, rank_62], 10, weights
        )
        for traces in ([lexical, rank_one, rank_62], [lexical, rank_62, rank_one]):
            self.assertGreater(
                replay_fusion.candidate_fusion_score(traces, 10, weights),
                only_deep,
            )

    def test_missing_rank_is_ignored(self) -> None:
        weights = IDENTIFIER_WEIGHTS
        trace = {"path": "fts", "route_id": "fts", "rank": None}
        self.assertEqual(
            replay_fusion.candidate_fusion_score([trace], 10, weights), 0.0
        )


class FusionWeightsTest(unittest.TestCase):
    def test_prefer_symbol_route_inherits_base_route_weights(self) -> None:
        weights = replay_fusion.FusionWeights(
            by_route={"vector": (1.0, 2.0)}, fallback=(1.1, 1.0)
        )
        self.assertEqual(weights.for_route("vector.prefer-symbol"), (1.0, 2.0))
        self.assertEqual(weights.for_route("unknown"), (1.1, 1.0))

    def test_weights_for_query_matches_the_shape_weight_table(self) -> None:
        # Mirrors QueryShape::fusion_weights (shape.rs:73-79); a typo here
        # would silently corrupt every replay.
        self.assertEqual(
            replay_fusion.weights_for_query("validateParameters").for_route("fts"),
            (1.1, 1.0),
        )
        self.assertEqual(
            replay_fusion.weights_for_query("validateParameters").for_route("vector"),
            (1.1, 1.0),
        )
        self.assertEqual(
            replay_fusion.weights_for_query("where is selection done").for_route("vector"),
            (1.0, 2.0),
        )
        self.assertEqual(
            replay_fusion.weights_for_query("where is validateParameters called")
            .for_route("fts"),
            (1.0, 1.0),
        )
        self.assertEqual(
            replay_fusion.weights_for_query("anything").for_route("fts-2"),
            (1.1, 1.0),
        )

    def test_natural_language_lets_a_deep_vector_hit_outrank_lexical_only(self) -> None:
        # Port of the Rust test: under natural-language weights a deep
        # vector hit outranks a lexical-only rank-1 hit; under identifier
        # weights the lexical preference is kept (pipeline.rs tests
        # natural_language_weights / identifier lexical preference).
        weights = replay_fusion.weights_for_query(
            "where does validation happen before execution"
        )
        lexical_only = [recall("fts", 1)]
        deep_vector = [recall("vector", 40)]
        self.assertGreater(
            replay_fusion.candidate_fusion_score(deep_vector, 10, weights),
            replay_fusion.candidate_fusion_score(lexical_only, 10, weights),
        )
        identifier_weights = replay_fusion.weights_for_query("validateParameters")
        self.assertGreater(
            replay_fusion.candidate_fusion_score(lexical_only, 10, identifier_weights),
            replay_fusion.candidate_fusion_score(deep_vector, 10, identifier_weights),
        )


def scored_recall(
    path: str, rank: int, score: float, route_id: str | None = None
) -> dict:
    return {
        "path": path,
        "route_id": route_id or path,
        "rank": rank,
        "score": score,
    }


class RouteScoreStatsTest(unittest.TestCase):
    """Per-query raw-score statistics over a candidate set (issue #70)."""

    def test_stats_cover_min_max_mean_std_per_path(self) -> None:
        candidates = [
            {"id": "a", "recall": [scored_recall("fts", 1, 6.0),
                                    scored_recall("vector", 1, 0.40)]},
            {"id": "b", "recall": [scored_recall("fts", 2, 4.0),
                                    scored_recall("vector", 2, 0.36)]},
            {"id": "c", "recall": [scored_recall("fts", 3, 5.0)]},
        ]
        stats = replay_fusion.route_score_stats(candidates)
        self.assertAlmostEqual(stats["fts"]["min"], 4.0, places=12)
        self.assertAlmostEqual(stats["fts"]["max"], 6.0, places=12)
        self.assertAlmostEqual(stats["fts"]["mean"], 5.0, places=12)
        self.assertAlmostEqual(stats["vector"]["min"], 0.36, places=12)
        self.assertAlmostEqual(stats["vector"]["max"], 0.40, places=12)
        # std over the two vector scores 0.36/0.40.
        self.assertAlmostEqual(stats["vector"]["std"], 0.02, places=12)

    def test_stats_ignore_entries_without_raw_score_or_rank(self) -> None:
        candidates = [
            {"id": "a", "recall": [
                {"path": "fts", "route_id": "fts", "rank": 1, "score": None},
                scored_recall("fts", 2, 5.0),
            ]},
        ]
        stats = replay_fusion.route_score_stats(candidates)
        self.assertAlmostEqual(stats["fts"]["min"], 5.0, places=12)
        self.assertAlmostEqual(stats["fts"]["max"], 5.0, places=12)

    def test_stats_missing_path_has_no_entry(self) -> None:
        candidates = [{"id": "a", "recall": [scored_recall("fts", 1, 5.0)]}]
        self.assertNotIn("vector", replay_fusion.route_score_stats(candidates))

    def test_stats_detect_distance_polarity(self) -> None:
        # The vector route's raw scores are cosine distances from the fp16
        # index: the best-ranked hit carries the LOWEST score, so the score
        # rises with rank and the route must be flipped before
        # normalization (verified against engine traces on
        # go-inventory-audit-v1: rank 1 = 0.2524 ... rank 44 = 0.5546).
        candidates = [
            {"id": "a", "recall": [scored_recall("vector", 1, 0.25)]},
            {"id": "b", "recall": [scored_recall("vector", 2, 0.30)]},
            {"id": "c", "recall": [scored_recall("vector", 3, 0.55)]},
        ]
        stats = replay_fusion.route_score_stats(candidates)
        self.assertFalse(stats["vector"]["higher_is_better"])
        # A similarity-style route (score falls with rank) stays unflipped.
        fts_candidates = [
            {"id": "a", "recall": [scored_recall("fts", 1, 12.2)]},
            {"id": "b", "recall": [scored_recall("fts", 2, 8.5)]},
            {"id": "c", "recall": [scored_recall("fts", 3, 3.4)]},
        ]
        stats = replay_fusion.route_score_stats(fts_candidates)
        self.assertTrue(stats["fts"]["higher_is_better"])

    def test_best_route_entry_picks_the_lowest_rank(self) -> None:
        # The route's own ordering is its quality ground truth, so the
        # best-ranked entry wins even when another entry of the same path
        # carries a higher raw score (the vector route's scores are
        # distances: lower is better).
        recall = [
            scored_recall("fts", 1, 4.0),
            scored_recall("fts", 5, 6.0, route_id="fts.prefer-symbol"),
        ]
        score, rank, route_id = replay_fusion.best_route_entry(recall)
        self.assertEqual((score, rank, route_id), (4.0, 1, "fts"))


class NormalizationVariantTest(unittest.TestCase):
    """Score-normalization fusion variants (issue #70)."""

    WEIGHTS = replay_fusion.FusionWeights(
        by_route={"fts": (1.0, 2.0), "vector": (1.0, 2.0)},
        fallback=(1.1, 1.0),
    )

    def candidates(self) -> list[dict]:
        return [
            {"id": "both", "recall": [scored_recall("fts", 3, 5.0),
                                      scored_recall("vector", 9, 0.38)]},
            {"id": "vector-only-top", "recall": [scored_recall("vector", 1, 0.40)]},
            {"id": "lexical-only-top", "recall": [scored_recall("fts", 1, 6.0)]},
            {"id": "vector-only-bottom", "recall": [scored_recall("vector", 20, 0.36)]},
        ]

    def scorer(self, name: str):
        return replay_fusion.make_query_variant(
            name, replay_fusion.route_score_stats(self.candidates())
        )

    def test_minmax_scales_routes_into_comparable_range(self) -> None:
        scorer = self.scorer("norm:minmax")
        both = scorer(
            [{"path": "fts", "route_id": "fts", "rank": 3, "score": 5.0},
             {"path": "vector", "route_id": "vector", "rank": 9, "score": 0.38}],
            10, self.WEIGHTS,
        )
        # fts scores over the set are {5.0, 6.0}: the candidate sits at the
        # route minimum (norm 0). vector scores are {0.36, 0.38, 0.40}: the
        # candidate sits mid-route (norm 0.5). NL weights are (1.0, 2.0).
        self.assertAlmostEqual(both, 1.0 * 0.0 + 2.0 * 0.5, places=12)
        # A route the candidate is absent from contributes exactly zero.
        vector_only_top = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.40}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(vector_only_top, 2.0 * 1.0, places=12)
        # The worst present candidate scores the same as an absent route.
        vector_only_bottom = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 20, "score": 0.36}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(vector_only_bottom, 0.0, places=12)

    def test_minmax_constant_route_scores_all_zero(self) -> None:
        candidates = [{"id": "a", "recall": [scored_recall("vector", 1, 0.4)]}]
        scorer = replay_fusion.make_query_variant(
            "norm:minmax", replay_fusion.route_score_stats(candidates)
        )
        value = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.4}],
            10, self.WEIGHTS,
        )
        self.assertEqual(value, 0.0)

    def test_minmax_inverts_distance_routes(self) -> None:
        candidates = [
            {"id": "a", "recall": [scored_recall("vector", 1, 0.25)]},
            {"id": "b", "recall": [scored_recall("vector", 2, 0.35)]},
            {"id": "c", "recall": [scored_recall("vector", 3, 0.55)]},
        ]
        scorer = replay_fusion.make_query_variant(
            "norm:minmax", replay_fusion.route_score_stats(candidates)
        )
        best = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.25}],
            10, self.WEIGHTS,
        )
        worst = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 3, "score": 0.55}],
            10, self.WEIGHTS,
        )
        # The smallest distance normalizes to the top of [0, 1].
        self.assertAlmostEqual(best, 2.0 * 1.0, places=12)
        self.assertAlmostEqual(worst, 0.0, places=12)

    def test_zscore_squashes_to_unit_interval(self) -> None:
        scorer = self.scorer("norm:zscore")
        import math

        # vector scores {0.36,0.38,0.40}: mean 0.38, std sqrt(8/3 e-4);
        # 0.40 -> z = 0.02/sqrt(8/3 * 1e-4) = sqrt(1.5).
        zv = 0.02 / math.sqrt(8.0 / 3.0 * 1e-4)
        expected_vector = 2.0 * (0.5 + 0.5 * zv / (1.0 + abs(zv)))
        value = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.40}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(value, expected_vector, places=12)
        self.assertGreater(value, 1.0)  # above the route mean
        # A below-mean hit scores below 1.0 (the absent-route baseline).
        low = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 20, "score": 0.36}],
            10, self.WEIGHTS,
        )
        self.assertLess(low, 1.0)
        self.assertGreater(low, 0.0)
        self.assertLess(expected_vector, 2.0)  # squash stays inside (0, 1)
        # fts scores {5.0, 6.0}: mean 5.5, std 0.5; 6.0 -> z = +1.
        fts_value = scorer(
            [{"path": "fts", "route_id": "fts", "rank": 1, "score": 6.0}],
            10, self.WEIGHTS,
        )
        expected_fts = 1.0 * (0.5 + 0.5 * 1.0 / 2.0)
        self.assertAlmostEqual(fts_value, expected_fts, places=12)

    def test_zscore_constant_route_scores_map_to_half(self) -> None:
        candidates = [{"id": "a", "recall": [scored_recall("vector", 1, 0.4)]}]
        scorer = replay_fusion.make_query_variant(
            "norm:zscore", replay_fusion.route_score_stats(candidates)
        )
        value = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.4}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(value, 2.0 * 0.5, places=12)

    def test_hybrid_mixes_normalized_raw_and_scaled_rank(self) -> None:
        scorer = self.scorer("norm:hybrid")
        # fts scores over the set are {5.0, 6.0}: rank-3 raw 5.0 normalizes
        # to 0.0; its rank term is (60+1)/(60+3). vector raw 0.38 normalizes
        # to 0.5; its rank term is (60+1)/(60+9).
        fts_term = 0.5 * 0.0 + 0.5 * ((60.0 + 1.0) / (60.0 + 3.0))
        vector_term = 0.5 * 0.5 + 0.5 * ((60.0 + 1.0) / (60.0 + 9.0))
        expected = 1.0 * fts_term + 2.0 * vector_term
        value = scorer(
            [{"path": "fts", "route_id": "fts", "rank": 3, "score": 5.0},
             {"path": "vector", "route_id": "vector", "rank": 9, "score": 0.38}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(value, expected, places=12)

    def test_absent_route_contributes_no_rank_term(self) -> None:
        scorer = self.scorer("norm:hybrid")
        value = scorer(
            [{"path": "vector", "route_id": "vector", "rank": 1, "score": 0.40}],
            10, self.WEIGHTS,
        )
        rank_term = (60.0 + 1.0) / (60.0 + 1.0)
        self.assertAlmostEqual(
            value, 2.0 * (0.5 * 1.0 + 0.5 * rank_term), places=12
        )

    def test_capped_corroboration_keeps_head_protection(self) -> None:
        scorer = self.scorer("norm:capped")
        # fts {5.0, 6.0} -> both's fts norm 0; vector {0.36, 0.38, 0.40} ->
        # both's vector norm 0.5. Weighted: primary = 2.0*0.5 = 1.0,
        # corroboration = 1.0*0.0 = 0.0 -> total 1.0.
        both = scorer(
            [{"path": "fts", "route_id": "fts", "rank": 3, "score": 5.0},
             {"path": "vector", "route_id": "vector", "rank": 9, "score": 0.38}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(both, 1.0, places=12)
        # A strongly corroborated candidate: primary 2.0*1.0 (vector top),
        # corroboration 1.0*1.0 (fts top) -> capped at 0.25 * primary.
        corroborated = scorer(
            [{"path": "fts", "route_id": "fts", "rank": 1, "score": 6.0},
             {"path": "vector", "route_id": "vector", "rank": 1, "score": 0.40}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(corroborated, 2.0 + 0.25 * 2.0, places=12)
        # The cap ratio is configurable.
        scorer_half = self.scorer("norm:capped:0.5")
        corroborated_half = scorer_half(
            [{"path": "fts", "route_id": "fts", "rank": 1, "score": 6.0},
             {"path": "vector", "route_id": "vector", "rank": 1, "score": 0.40}],
            10, self.WEIGHTS,
        )
        self.assertAlmostEqual(corroborated_half, 2.0 + 0.5 * 2.0, places=12)

    def test_make_query_variant_rejects_unknown_names(self) -> None:
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:nope", {})

    def test_make_query_variant_rejects_malformed_specs(self) -> None:
        # The numeric parameter applies to hybrid/capped only, must parse,
        # and must be finite: NaN would silently poison every score.
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:minmax:0.3", {})
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:hybrid:abc", {})
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:capped:xyz", {})
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:hybrid:nan", {})
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:hybrid:inf", {})
        with self.assertRaises(SystemExit):
            replay_fusion.make_query_variant("norm:capped:-inf", {})

    def test_normalization_variants_need_stats(self) -> None:
        # A normalization variant cannot be built without per-query stats;
        # silently defaulting them would produce meaningless numbers.
        with self.assertRaises(ValueError):
            replay_fusion.make_query_variant("norm:minmax", None)


class TraceDumpRoundtripTest(unittest.TestCase):
    def test_dump_and_load_preserve_candidates(self) -> None:
        import tempfile

        rows = [{
            "id": "q1",
            "query": "where is validation done",
            "candidates": [{
                "id": "hit-1",
                "unit_ids": ["a.go::A"],
                "recall": [scored_recall("fts", 1, 5.5)],
                "engine_fused_score": 0.0178,
            }],
        }]
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "traces.json"
            replay_fusion.dump_traces(path, rows, {"deep_limit": 200})
            loaded = replay_fusion.load_traces(path)
            self.assertEqual(loaded, rows)


class ReplayRankingTest(unittest.TestCase):
    def test_replay_ranks_and_ties_like_the_engine(self) -> None:
        # Equal scores are broken by entity id ascending, matching the
        # engine's sort_by on (score desc, id asc).
        candidates = [
            {"id": "b", "recall": [recall("fts", 1)]},
            {"id": "a", "recall": [recall("fts", 1)]},
            {"id": "c", "recall": [recall("fts", 2)]},
        ]
        ranked = replay_fusion.replay_ranking(candidates, 10, IDENTIFIER_WEIGHTS)
        self.assertEqual([c["id"] for c in ranked], ["a", "b", "c"])


class ValidateArgsTest(unittest.TestCase):
    """The --traces-from / collection argument guards (main() entry)."""

    def make_parser(self):
        class StubParser:
            def __init__(self) -> None:
                self.errors: list[str] = []

            def error(self, message: str):
                self.errors.append(message)
                raise SystemExit(2)

        return StubParser()

    def args(self, **overrides):
        from argparse import Namespace

        base = {
            "traces_from": None, "binary": None, "work_root": None,
            "dump_traces": False,
        }
        base.update(overrides)
        return Namespace(**base)

    def test_collection_requires_binary_and_work_root(self) -> None:
        parser = self.make_parser()
        with self.assertRaises(SystemExit):
            replay_fusion.validate_args(parser, self.args())
        self.assertIn("required", parser.errors[0])

    def test_traces_from_rejects_binary_or_work_root(self) -> None:
        parser = self.make_parser()
        with self.assertRaises(SystemExit):
            replay_fusion.validate_args(
                parser, self.args(traces_from=Path("/tmp/x"), binary=Path("zg"))
            )
        self.assertIn("cannot be combined", parser.errors[0])

    def test_traces_from_rejects_dump_traces(self) -> None:
        parser = self.make_parser()
        with self.assertRaises(SystemExit):
            replay_fusion.validate_args(
                parser,
                self.args(traces_from=Path("/tmp/x"), dump_traces=True),
            )
        self.assertIn("requires collecting", parser.errors[0])

    def test_valid_combinations_pass(self) -> None:
        replay_fusion.validate_args(
            self.make_parser(),
            self.args(binary=Path("zg"), work_root=Path("/tmp/root")),
        )
        replay_fusion.validate_args(
            self.make_parser(), self.args(traces_from=Path("/tmp/out"))
        )


if __name__ == "__main__":
    unittest.main()
