#!/usr/bin/env python3
"""Unit tests for the vendored replay normalization helpers.

Run: python3 -m unittest discover -s benchmarks/quality-lanes -p 'test_*.py'
"""
from __future__ import annotations

import json
import pathlib
import tempfile
import unittest

import replay_normalization as rn


BRACELESS_ABOVE_BRACED = '''package demo

// maxBudget bounds a single audit pass.
const maxBudget = 32

func audit(items []string) bool {
	if len(items) > maxBudget {
		return false
	}
	return true
}
'''


class BalancedEndTest(unittest.TestCase):
    def test_braceless_declaration_ends_on_its_own_line(self) -> None:
        lines = BRACELESS_ABOVE_BRACED.splitlines()
        start = next(
            i + 1 for i, line in enumerate(lines) if line.startswith("const maxBudget")
        )
        end = rn._balanced_end(lines, start)
        self.assertEqual(start, end)

    def test_braced_declaration_spans_to_closing_brace(self) -> None:
        lines = BRACELESS_ABOVE_BRACED.splitlines()
        start = next(
            i + 1 for i, line in enumerate(lines) if line.startswith("func audit")
        )
        end = rn._balanced_end(lines, start)
        closing = next(
            i + 1 for i, line in enumerate(lines) if line == "}"
        )
        self.assertEqual(closing, end)

    def test_unit_spans_do_not_bridge_declarations(self) -> None:
        manifest = {"units": [
            {"unit_id": "u-const", "path": "demo.go", "symbol": "maxBudget"},
            {"unit_id": "u-func", "path": "demo.go", "symbol": "audit"},
        ]}
        with tempfile.TemporaryDirectory() as tmp:
            fixture = pathlib.Path(tmp)
            (fixture / "demo.go").write_text(BRACELESS_ABOVE_BRACED, encoding="utf-8")
            spans = rn._unit_spans(fixture, manifest)
        by_id = {s.unit_id: s for s in spans}
        self.assertEqual(by_id["u-const"].end, by_id["u-const"].start)
        self.assertLess(by_id["u-const"].end, by_id["u-func"].start)


class CopyFixtureSourcesTest(unittest.TestCase):
    def _fixture(self, tmp: pathlib.Path) -> pathlib.Path:
        fixture = tmp / "fixture"
        fixture.mkdir()
        manifest = {
            "source": {"include": ["**/*.go"], "exclude": []},
            "units": [],
        }
        (fixture / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        (fixture / "internal").mkdir()
        (fixture / "internal" / "demo.go").write_text("package demo\n", encoding="utf-8")
        return fixture

    def test_reuses_marked_root_and_refuses_foreign_content(self) -> None:
        manifest = {"source": {"include": ["**/*.go"], "exclude": []}, "units": []}
        with tempfile.TemporaryDirectory() as tmp:
            fixture = self._fixture(pathlib.Path(tmp))
            root = pathlib.Path(tmp) / "work"
            rn.copy_fixture_sources(fixture, manifest, root)
            self.assertTrue((root / ".quality-lane-root").exists())
            # A second run on the marked root succeeds (fresh copy).
            (root / "internal" / "demo.go").write_text("package demo\n", encoding="utf-8")
            rn.copy_fixture_sources(fixture, manifest, root)
            self.assertEqual(
                (root / "internal" / "demo.go").read_text(encoding="utf-8"),
                "package demo\n",
            )
            # A foreign file in an unmarked directory is refused.
            foreign = pathlib.Path(tmp) / "foreign"
            foreign.mkdir()
            (foreign / "important.txt").write_text("keep me", encoding="utf-8")
            with self.assertRaises(rn.ReplayError):
                rn.copy_fixture_sources(fixture, manifest, foreign)
            self.assertEqual(
                (foreign / "important.txt").read_text(encoding="utf-8"), "keep me"
            )


if __name__ == "__main__":
    unittest.main()
