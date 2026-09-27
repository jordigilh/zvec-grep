"""Offline guards for the Go-only candidate-pool diagnosis."""
import importlib.util
import sys
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "go_retrieval_timebox", Path(__file__).resolve().parents[2] / "scripts/go_retrieval_timebox.py"
)
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class CandidateInventoryTest(unittest.TestCase):
    def test_positive_units_are_keyed_by_query_and_split_into_index_pool_and_order(self):
        queries = [{"id": "first", "judgments": [
            {"unit_id": unit, "grade": 2} for unit in ("found", "tail", "miss", "unindexed")
        ]}, {"id": "second", "judgments": [{"unit_id": "found", "grade": 3}]}]
        indexed = ["found", "tail", "miss"]
        baseline = {"first": ["found"], "second": []}
        pool = {"first": ["miss", "found", "tail"], "second": ["found"]}
        result = runner.positive_rank_inventory(queries, indexed, baseline, pool)
        by_id = {row["unit_id"]: row for row in result["first"]}
        self.assertEqual(by_id["found"]["category"], "in_baseline_top_10")
        self.assertEqual(by_id["tail"]["category"], "in_pool_below_10")
        self.assertEqual(by_id["miss"]["pool_rank"], 1)
        self.assertEqual(by_id["unindexed"]["category"], "not_eligible_in_index")
        self.assertEqual(result["second"][0]["category"], "in_pool_below_10")
        with self.assertRaises(ValueError):
            runner.positive_rank_inventory(queries, indexed, baseline, {"first": []})

    def test_stale_or_duplicate_ranks_cannot_be_credited(self):
        queries = [{"id": "q", "judgments": [{"unit_id": "a", "grade": 2}]}]
        with self.assertRaises(ValueError):
            runner.positive_rank_inventory(queries, ["a"], {"q": ["a", "a"]}, {"q": ["a"]})
        with self.assertRaises(ValueError):
            runner.positive_rank_inventory(queries, [], {"q": ["a"]}, {"q": []})


if __name__ == "__main__":
    unittest.main()
