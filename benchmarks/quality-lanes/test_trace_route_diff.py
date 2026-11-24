#!/usr/bin/env python3
"""Tests for trace_route_diff.py (route trace-diff tool for the quality lanes).

Covers the pure functions only: the QueryShape port (mirrors the Rust tests in
rust/crates/zg-engine/src/pipelines/indexed_search/shape.rs), the matchedBy
header scanner, the per-hit trace-line parser, and the missing-relevant-unit
analysis. The route runners need a built zg binary and are exercised by the
documented manual invocation, not by this suite.
"""
from __future__ import annotations

import importlib
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
trace_route_diff = importlib.import_module("trace_route_diff")


class QueryShapePortTest(unittest.TestCase):
    """Mirrors the four Rust test cases in shape.rs (lines 224-277)."""

    def test_classifies_identifier_queries(self) -> None:
        for query in [
            "validateParameters",
            "Workflows.recordDiscoveredWorkflows",
            "list_workflows",
        ]:
            self.assertEqual(
                trace_route_diff.classify(query), "identifier", query
            )

    def test_classifies_natural_language_queries(self) -> None:
        for query in [
            "Where are remediation workflow parameters validated before execution?",
            "How does selection work",
            "recording the discovered workflows somewhere",
        ]:
            self.assertEqual(
                trace_route_diff.classify(query), "natural_language", query
            )

    def test_classifies_short_and_mixed_queries(self) -> None:
        self.assertEqual(trace_route_diff.classify("empty discovery state"), "mixed")
        self.assertEqual(
            trace_route_diff.classify("where is validateParameters called"), "mixed"
        )
        self.assertEqual(trace_route_diff.classify(""), "identifier")
        self.assertEqual(
            trace_route_diff.classify("ワークフローはどこで検証されますか"),
            "natural_language",
        )

    def test_identifier_shape_rules(self) -> None:
        # Internal separator with alphanumeric on both sides.
        self.assertEqual(trace_route_diff.classify("a/b"), "identifier")
        self.assertEqual(trace_route_diff.classify("pkg.Name"), "identifier")
        # A single token (even a capitalized word) is an identifier, matching
        # the Rust single-token rule.
        self.assertEqual(trace_route_diff.classify("Where"), "identifier")
        # In a multi-token query a leading capital alone is not identifier
        # shaped, so the query is not an identifier query.
        self.assertEqual(trace_route_diff.classify("Record assets"), "mixed")
        # Camel case boundary marks an identifier token.
        self.assertEqual(trace_route_diff.classify("fooBar"), "identifier")
        # PascalCase run followed by lowercase marks a boundary.
        self.assertEqual(trace_route_diff.classify("HTTPServer"), "identifier")
        # A single ASCII token is an identifier.
        self.assertEqual(trace_route_diff.classify("scan"), "identifier")

    def test_natural_language_min_tokens(self) -> None:
        # Three plain words with no stopword and no '?' stay mixed.
        self.assertEqual(trace_route_diff.classify("record asset ids"), "mixed")
        # Four plain words become natural language.
        self.assertEqual(
            trace_route_diff.classify("record scanned asset ids"), "natural_language"
        )


class MatchedByScannerTest(unittest.TestCase):
    def test_extracts_matched_by_per_rank(self) -> None:
        stdout = "\n".join([
            "#1 matchedBy=fts+vector internal/a.go:1-10",
            "symbol: function Add",
            "#2 matchedBy=vector internal/b.go:2-4",
            "symbol: class B",
            "#3 internal/c.go:5-5",
            "symbol: function C",
        ])
        self.assertEqual(
            trace_route_diff.scan_matched_by(stdout),
            {1: "fts+vector", 2: "vector"},
        )


class TraceLineParserTest(unittest.TestCase):
    def test_parses_trace_json_after_hit(self) -> None:
        stdout = "\n".join([
            "#1 matchedBy=fts+vector internal/a.go:1-10",
            "symbol: function Add",
            "score: 0.0373",
            'trace: {"recall":[{"path":"fts","route_id":"fts","query":"q",'
            '"found":true,"rank":13,"score":5.8,"forced":false,"reason":null},'
            '{"path":"vector","route_id":"vector","query":"q","found":true,'
            '"rank":7,"score":0.37,"forced":false,"reason":null}],'
            '"fusion":{"rank":1,"score":0.0373,"forced":false},'
            '"final_selection":{"returned_by_limit":true,"cutoff_rank":3}}',
            "#2 internal/b.go:2-4",
            "symbol: class B",
        ])
        traces = trace_route_diff.parse_hit_traces(stdout)
        self.assertEqual(set(traces), {1})
        recall = traces[1]["recall"]
        self.assertEqual(recall[0]["path"], "fts")
        self.assertEqual(recall[0]["rank"], 13)
        self.assertEqual(recall[1]["rank"], 7)
        self.assertAlmostEqual(traces[1]["fusion"]["score"], 0.0373)


class MissingRelevantAnalysisTest(unittest.TestCase):
    def test_missing_units_and_route_ranks(self) -> None:
        qrels = {"queries": [{"id": "q1", "judgments": [
            {"unit_id": "a::A", "grade": 3, "rationale": ""},
            {"unit_id": "b::B", "grade": 2, "rationale": ""},
            {"unit_id": "c::C", "grade": 1, "rationale": ""},
            {"unit_id": "d::D", "grade": 0, "rationale": ""},
        ]}]}
        hybrid = ["x::X", "a::A"]
        fts = ["a::A", "b::B"]
        vec = ["x::X", "c::C"]
        analysis = trace_route_diff.analyze_query(
            query_id="q1",
            query_text="where is b",
            hybrid=hybrid,
            fts=fts,
            vector=vec,
            grades={j["unit_id"]: j["grade"] for j in qrels["queries"][0]["judgments"]},
        )
        # b::B (grade 2) is missing from hybrid and sits at lexical rank 2,
        # absent from the vector top-20. c::C is grade 1: below the
        # recall-relevant threshold, so it is excluded entirely.
        missing = {m["unit_id"]: m for m in analysis["missing_relevant"]}
        self.assertIn("b::B", missing)
        self.assertNotIn("c::C", missing)
        self.assertEqual(missing["b::B"]["lexical_rank"], 2)
        self.assertIsNone(missing["b::B"]["vector_rank"])
        self.assertEqual(analysis["recall_relevant_count"], 2)
        self.assertEqual(analysis["recall_relevant_found"], 1)


class MarkdownReportTest(unittest.TestCase):
    def test_renders_per_query_sections(self) -> None:
        report = [{
            "id": "q1",
            "query": "Where are ids recorded?",
            "shape": "natural_language",
            "recall_relevant_count": 2,
            "recall_relevant_found": 1,
            "missing_relevant": [{
                "unit_id": "b::B", "grade": 2,
                "lexical_rank": 2, "vector_rank": None,
            }],
        }]
        markdown = trace_route_diff.render_markdown(
            {"fixture_id": "fx", "resolved_model": "local/m"}, report
        )
        self.assertIn("q1", markdown)
        self.assertIn("b::B", markdown)
        self.assertIn("local/m", markdown)


if __name__ == "__main__":
    unittest.main()
