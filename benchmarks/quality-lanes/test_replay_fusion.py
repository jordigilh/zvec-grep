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


if __name__ == "__main__":
    unittest.main()
