import unittest

from scripts.replay_ir_projection_qeval import compare_metrics


class GoIrQevalGateTests(unittest.TestCase):
    def test_improvement_requires_no_aggregate_metric_regression(self):
        metrics = {
            "runs": [
                {
                    "backend": "syntax-only",
                    "overall": {
                        "ndcg@10": 0.5,
                        "mrr@10": 0.6,
                        "recall@10": 0.4,
                        "precision@10": 0.2,
                    },
                    "per_query": [{"id": "q1", "ndcg@10": 0.5, "mrr@10": 1.0, "recall@10": 0.5, "precision@10": 0.1}],
                },
                {
                    "backend": "code-ir-projection",
                    "overall": {
                        "ndcg@10": 0.51,
                        "mrr@10": 0.6,
                        "recall@10": 0.4,
                        "precision@10": 0.2,
                    },
                    "per_query": [{"id": "q1", "ndcg@10": 0.51, "mrr@10": 1.0, "recall@10": 0.5, "precision@10": 0.1}],
                },
            ]
        }
        comparison = compare_metrics(metrics)["comparisons"][0]
        self.assertTrue(comparison["no_regression_gate"])
        self.assertTrue(comparison["improvement_gate"])

        metrics["runs"][1]["overall"]["recall@10"] = 0.39
        comparison = compare_metrics(metrics)["comparisons"][0]
        self.assertFalse(comparison["no_regression_gate"])
        self.assertTrue(comparison["improvement_gate"])

    def test_neutral_candidate_does_not_pass_improvement_gate(self):
        baseline = {
            "backend": "syntax-only",
            "overall": {key: 0.5 for key in ("ndcg@10", "mrr@10", "recall@10", "precision@10")},
            "per_query": [{"id": "q1", "ndcg@10": 0.5, "mrr@10": 0.5, "recall@10": 0.5, "precision@10": 0.5}],
        }
        candidate = {**baseline, "backend": "code-ir-projection"}
        comparison = compare_metrics({"runs": [baseline, candidate]})["comparisons"][0]
        self.assertTrue(comparison["no_regression_gate"])
        self.assertFalse(comparison["improvement_gate"])


if __name__ == "__main__":
    unittest.main()
