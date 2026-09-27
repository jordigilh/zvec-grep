"""Offline controls for the frozen v2 retrieval runner (no model needed)."""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "code_ir_v2_qeval", REPO / "scripts/replay_code_ir_v2_qeval.py"
)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def metric_row(query_id, ndcg):
    return {
        "id": query_id,
        "ndcg@10": ndcg,
        "mrr@10": 0.5,
        "recall@10": 0.25,
        "precision@10": 0.1,
    }


class QevalContractTest(unittest.TestCase):
    def test_factorial_arms_require_exact_names_and_frozen_query_order(self):
        ids = [f"q{i}" for i in range(8)]
        names = runner.FACTORIAL_ARMS
        arms = [
            {"name": name, "queries": [{"id": query_id, "query": query_id} for query_id in ids]}
            for name in names
        ]
        runner.validate_arms(arms, ids, ids, expected_names=names)
        for altered in (
            arms[:-1], [*arms[:-1], arms[-2]],
            [*arms[:-1], {**arms[-1], "queries": list(reversed(arms[-1]["queries"]))}],
        ):
            with self.assertRaises(ValueError):
                runner.validate_arms(altered, ids, ids, expected_names=names)

    def test_factorial_comparisons_track_both_main_effects_without_changing_baseline(self):
        ids = [f"q{i}" for i in range(8)]
        names = runner.FACTORIAL_ARMS
        values = [0.5, 0.45, 0.48, 0.52, 0.55]
        runs = []
        for name, value in zip(names, values):
            per_query = [metric_row(query_id, value) for query_id in ids]
            runs.append({
                "backend": name, "per_query": per_query,
                "overall": {
                    key: sum(row[key] for row in per_query) / 8
                    for key in per_query[0] if key != "id"
                },
            })
        result = runner.compare_ablation_metrics({"runs": runs}, "go")
        self.assertAlmostEqual(result["vs_control"]["code-ir-v2"]["aggregate_deltas"]["ndcg@10"], 0.05)
        self.assertAlmostEqual(result["factor_effects"]["policy_with_v1_text"]["aggregate_deltas"]["ndcg@10"], 0.03)
        self.assertAlmostEqual(result["factor_effects"]["metadata_with_v1_units"]["aggregate_deltas"]["ndcg@10"], 0.07)
        self.assertAlmostEqual(result["factor_effects"]["metadata_with_v2_units"]["aggregate_deltas"]["ndcg@10"], 0.07)
        self.assertAlmostEqual(result["factor_effects"]["policy_with_v2_text"]["aggregate_deltas"]["ndcg@10"], 0.03)
        self.assertEqual(len(result["factor_effects"]["policy_with_v1_text"]["per_query_deltas"]), 8)

    def test_arms_reject_missing_duplicate_or_reordered_queries(self):
        ids = [f"q{i}" for i in range(8)]
        arms = [
            {"name": "syntax-only", "queries": [{"id": query_id, "hits": []} for query_id in ids]},
            {"name": "code-ir-v2", "queries": [{"id": query_id, "hits": []} for query_id in ids]},
        ]
        runner.validate_arms(arms, ids)
        for bad in (list(reversed(ids)), [*ids[:-1], ids[0]], ids[:-1]):
            altered = json.loads(json.dumps(arms))
            altered[1]["queries"] = [{"id": query_id, "hits": []} for query_id in bad]
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                runner.validate_arms(altered, ids)
        text = [f"query {i}" for i in range(8)]
        for arm in arms:
            for row, query in zip(arm["queries"], text):
                row["query"] = query
        runner.validate_arms(arms, ids, text)
        arms[1]["queries"][2]["query"] = "changed query"
        with self.assertRaises(ValueError):
            runner.validate_arms(arms, ids, text)

    def test_normalization_keeps_rank_order_and_rejects_invalid_hits(self):
        arm = {"name": "code-ir-v2", "queries": [{"id": "q1", "hits": [
            {"rank": 1, "path": "a.go", "start_line": 1, "end_line": 2},
            {"rank": 2, "path": "a.go", "start_line": 3, "end_line": 4},
        ]}]}
        def map_chunk(_units, _path, start, _end):
            return [f"u{start}"]

        def dedupe(rows):
            return [{"unit_id": item} for _, unit_ids in rows for item in unit_ids]

        normalized = runner.normalize_arm(arm, [], map_chunk, dedupe)
        self.assertEqual(
            normalized["queries"][0]["results"],
            [{"unit_id": "u1"}, {"unit_id": "u3"}],
        )
        for hits in (
            [{**arm["queries"][0]["hits"][0], "rank": 2}],
            [{**arm["queries"][0]["hits"][0], "path": "missing.go"}],
        ):
            bad = json.loads(json.dumps(arm))
            bad["queries"][0]["hits"] = hits
            with self.assertRaises(ValueError):
                runner.normalize_arm(
                    bad, [], lambda units, path, start, end: [] if path == "missing.go" else map_chunk(units, path, start, end), dedupe
                )

    def test_gate_and_query_losses_are_keyed_not_zipped(self):
        baseline = [metric_row("q1", 0.8), metric_row("q2", 0.2), *[metric_row(f"q{i}", 0.5) for i in range(3, 9)]]
        candidate = [metric_row("q2", 0.4), metric_row("q1", 0.6), *[metric_row(f"q{i}", 0.5) for i in range(3, 9)]]
        metrics = {"runs": [
            {"backend": "syntax-only", "overall": {key: sum(row[key] for row in baseline)/8 for key in baseline[0] if key != "id"}, "per_query": baseline},
            {"backend": "code-ir-v2", "overall": {key: sum(row[key] for row in candidate)/8 for key in baseline[0] if key != "id"}, "per_query": candidate},
        ]}
        comparison = runner.compare_metrics(metrics, "go")
        self.assertTrue(comparison["no_regression_gate"])
        self.assertFalse(comparison["improvement_gate"])
        self.assertFalse(comparison["enablement_gate"])
        self.assertEqual(comparison["query_losses"][0]["id"], "q1")
        self.assertAlmostEqual(comparison["per_query_deltas"][0]["ndcg@10"], -0.2)
        metrics["runs"][1]["overall"]["ndcg@10"] -= 0.001
        self.assertFalse(runner.compare_metrics(metrics, "go")["enablement_gate"])
        metrics["runs"][1]["overall"]["ndcg@10"] += 0.002
        self.assertTrue(runner.compare_metrics(metrics, "go")["enablement_gate"])
        metrics["runs"][1]["per_query"][-1]["id"] = "q3"
        with self.assertRaises(ValueError):
            runner.compare_metrics(metrics, "go")

    def test_fixture_staging_rejects_changed_source_and_never_writes_fixture(self):
        with tempfile.TemporaryDirectory() as parent:
            root = Path(parent)
            fixture = root / "fixture"
            fixture.mkdir()
            (fixture / "a.go").write_bytes(b"package main\n")
            baseline, candidate = root / "baseline", root / "candidate"
            for path in (baseline, candidate):
                path.mkdir()
            manifest = {"units": [{"path": "a.go"}]}
            runner.stage_sources(fixture, manifest, (baseline, candidate), 13)
            self.assertEqual((fixture / "a.go").read_bytes(), (candidate / "a.go").read_bytes())
            (fixture / "a.go").write_bytes(b"changed")
            with self.assertRaises(ValueError):
                runner.stage_sources(fixture, manifest, (baseline, candidate), 13)
            self.assertEqual((candidate / "a.go").read_bytes(), b"package main\n")
            fresh = root / "fresh"
            fresh.mkdir()
            with self.assertRaises(ValueError):
                runner.stage_sources(fixture, manifest, (fresh, fresh), 13)
            self.assertFalse((fresh / "a.go").exists())


if __name__ == "__main__":
    unittest.main()
