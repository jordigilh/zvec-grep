#!/usr/bin/env python3
"""Negative tests for the multi-language lane's checks and parser.

Every guard added to ``check_multilanguage_fixtures.py``,
``check_baselines_multilanguage.py`` and ``run_qeval_multilanguage.py`` is
exercised here with input that must be rejected, so a future edit cannot make
the guard vacuous without failing this suite.

Run: python3 -m unittest discover -s benchmarks/quality-lanes -p 'test_*.py'
"""
from __future__ import annotations

import copy
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from run_qeval_multilanguage import parse_preview  # noqa: E402
from replay_normalization import ReplayError  # noqa: E402

FIXTURE = "demo-workflow-discovery-v1"
SNAPSHOT = "a" * 64
QUERY_TEXT = "Where does demo live?"
UNITS = [
    {"unit_id": "a.py::A", "path": "a.py", "symbol": "A", "start_line": 10, "end_line": 20},
    {"unit_id": "a.py::B", "path": "a.py", "symbol": "B", "start_line": 30, "end_line": 40},
]
OVERALL = {
    "ndcg@10": 0.5,
    "mrr@10": 0.5,
    "recall@10": 0.5,
    "precision@10": 0.5,
}


def fixture_documents() -> tuple[dict, dict, dict]:
    manifest = {
        "schema_version": 1,
        "fixture_id": FIXTURE,
        "language": "python",
        "units": copy.deepcopy(UNITS),
    }
    truth = {
        "suite_id": "suite",
        "queries": [
            {"id": "q1", "query": QUERY_TEXT, "category": "c", "judgments": []},
        ],
    }
    qrels = {
        "suite_id": "suite",
        "judgment_status": "adjudicated",
        "candidate_pool_complete": True,
        "unit_count": len(UNITS),
        "source": {"snapshot_sha256": SNAPSHOT},
        "queries": [
            {
                "id": "q1",
                "query": QUERY_TEXT,
                "category": "c",
                "judgments": [
                    {"unit_id": "a.py::A", "grade": 3},
                    {"unit_id": "a.py::B", "grade": 0},
                ],
            },
        ],
    }
    return manifest, truth, qrels


def baselines_document(**overrides) -> dict:
    entry = {
        "role": "tuning",
        "retrieval": dict(OVERALL),
        "fixture_snapshot_sha256": SNAPSHOT,
    }
    entry.update(overrides)
    return {
        "schema": "quality-lanes-baselines-multilanguage-v1",
        "fixtures": {FIXTURE: entry} if entry else {},
    }


class FixtureCase(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        self.fixtures = self.root / "fixtures"
        self.fixture = self.fixtures / FIXTURE
        self.fixture.mkdir(parents=True)
        self.baselines = self.root / "baselines.json"
        manifest, truth, qrels = fixture_documents()
        self.manifest = manifest
        self.truth = truth
        self.qrels = qrels
        self.write_fixture()

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write_fixture(self) -> None:
        for name, document in (
            ("manifest.json", self.manifest),
            ("truth.json", self.truth),
            ("qrels.json", self.qrels),
        ):
            (self.fixture / name).write_text(json.dumps(document), encoding="utf-8")

    def write_baselines(self, document: dict) -> None:
        self.baselines.write_text(json.dumps(document), encoding="utf-8")

    def run_checker(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(HERE / "check_multilanguage_fixtures.py"),
                "--baselines", str(self.baselines),
                "--fixtures", str(self.fixtures),
                *extra,
            ],
            capture_output=True,
            text=True,
            check=False,
        )


class FixtureCheckTest(FixtureCase):
    def test_valid_fixture_passes(self) -> None:
        self.write_baselines(baselines_document())
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_dropped_judgment_fails(self) -> None:
        # The recall denominator is derived from judged grades, so a dropped
        # judgment silently inflates recall when the unit is not retrieved.
        self.qrels["queries"][0]["judgments"] = [{"unit_id": "a.py::A", "grade": 3}]
        self.write_fixture()
        self.write_baselines(baselines_document())
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 1)
        self.assertIn("no qrels judgment", completed.stderr)

    def test_duplicate_judgment_fails(self) -> None:
        self.qrels["queries"][0]["judgments"].append({"unit_id": "a.py::A", "grade": 2})
        self.write_fixture()
        self.write_baselines(baselines_document())
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 1)
        self.assertIn("duplicate qrels judgments", completed.stderr)

    def test_truth_qrels_query_text_drift_fails(self) -> None:
        # The runner searches truth while the scorer grades against qrels; a
        # text drift would score a different query than the one adjudicated.
        self.truth["queries"][0]["query"] = "Some other query"
        self.write_fixture()
        self.write_baselines(baselines_document())
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 1)
        self.assertIn("query text differ", completed.stderr)

    def test_snapshot_mismatch_fails(self) -> None:
        self.qrels["source"]["snapshot_sha256"] = "b" * 64
        self.write_fixture()
        self.write_baselines(baselines_document())
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 1)
        self.assertIn("does not match pinned", completed.stderr)

    def test_missing_pinned_snapshot_fails(self) -> None:
        self.write_baselines(baselines_document(fixture_snapshot_sha256=None))
        completed = self.run_checker()
        self.assertEqual(completed.returncode, 1)
        self.assertIn("no fixture_snapshot_sha256", completed.stderr)


class BaselineCheckTest(FixtureCase):
    def write_results(self, overall: dict, snapshot: str = SNAPSHOT) -> pathlib.Path:
        results = self.root / "metrics.json"
        results.write_text(
            json.dumps({
                "fixture_id": FIXTURE,
                "fixture": {"snapshot_sha256": snapshot},
                "overall": overall,
            }),
            encoding="utf-8",
        )
        return results

    def run_gate(self, results: pathlib.Path) -> subprocess.CompletedProcess[str]:
        self.write_baselines(baselines_document())
        return subprocess.run(
            [
                sys.executable,
                str(HERE / "check_baselines_multilanguage.py"),
                "--baselines", str(self.baselines),
                "--fixture", FIXTURE,
                "--results", str(results),
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_equal_floors_pass(self) -> None:
        completed = self.run_gate(self.write_results(OVERALL))
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_improvement_passes_and_is_printed(self) -> None:
        better = {metric: value + 0.1 for metric, value in OVERALL.items()}
        completed = self.run_gate(self.write_results(better))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("improvement", completed.stdout)

    def test_regression_on_one_metric_fails(self) -> None:
        worse = dict(OVERALL)
        worse["recall@10"] = OVERALL["recall@10"] - 0.01
        completed = self.run_gate(self.write_results(worse))
        self.assertEqual(completed.returncode, 1)
        self.assertIn("recall@10", completed.stderr)

    def test_result_snapshot_mismatch_fails(self) -> None:
        completed = self.run_gate(self.write_results(OVERALL, snapshot="b" * 64))
        self.assertEqual(completed.returncode, 2)
        self.assertIn("does not match pinned", completed.stderr)


class ParsePreviewTest(unittest.TestCase):
    def test_symbol_line_maps_to_manifest_unit(self) -> None:
        ranked, unmatched, summary = parse_preview(
            "#1 matchedBy=vector a.py:10-20\nsymbol: function A\n", UNITS
        )
        self.assertEqual([row["unit_id"] for row in ranked], ["a.py::A"])
        self.assertEqual(ranked[0]["resolved_by"], "symbol")
        self.assertEqual(unmatched, [])
        self.assertEqual(summary["resolved_by_symbol"], 1)

    def test_symbolless_hit_falls_back_to_span(self) -> None:
        ranked, unmatched, summary = parse_preview(
            "#1 matchedBy=vector a.py:32-35\n", UNITS
        )
        self.assertEqual([row["unit_id"] for row in ranked], ["a.py::B"])
        self.assertEqual(summary["resolved_by_span"], 1)
        self.assertEqual(unmatched, [])

    def test_unresolvable_hit_is_reported_not_dropped(self) -> None:
        ranked, unmatched, _ = parse_preview(
            "#1 matchedBy=vector b.py:1-2\nsymbol: function Nope\n", UNITS
        )
        self.assertEqual(ranked, [])
        self.assertEqual(len(unmatched), 1)
        self.assertIn("symbol and span both failed", unmatched[0]["reason"])

    def test_non_source_header_is_reported(self) -> None:
        ranked, unmatched, _ = parse_preview(
            "#1 matchedBy=vector a.py:file\n", UNITS
        )
        self.assertEqual(ranked, [])
        self.assertEqual(len(unmatched), 1)
        self.assertIn("not a source span", unmatched[0]["reason"])

    def test_empty_result_label_is_a_successful_empty_ranking(self) -> None:
        ranked, unmatched, summary = parse_preview("No matches.\n", UNITS)
        self.assertEqual(ranked, [])
        self.assertEqual(unmatched, [])
        self.assertEqual(summary["unmatched"], 0)

    def test_no_output_is_an_error(self) -> None:
        with self.assertRaises(ReplayError):
            parse_preview("", UNITS)


if __name__ == "__main__":
    unittest.main()
