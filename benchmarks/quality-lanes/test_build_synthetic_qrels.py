#!/usr/bin/env python3
"""Tests for tools/build_synthetic_qrels.py (vendored from engram).

The vendored builder must reproduce an existing lane's committed qrels.json
byte-for-byte, which pins both the complete-universe expansion and the
snapshot digest convention (sha256 over ``path\\0content\\0`` per sorted
manifest-selected file).
"""
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

LANES = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location(
    "build_synthetic_qrels", LANES / "tools" / "build_synthetic_qrels.py"
)
build_synthetic_qrels = importlib.util.module_from_spec(_spec)
sys.modules["build_synthetic_qrels"] = build_synthetic_qrels
_spec.loader.exec_module(build_synthetic_qrels)


class VendoredBuilderTest(unittest.TestCase):
    def test_reproduces_python_lane_qrels_byte_for_byte(self) -> None:
        fixture = LANES / "fixtures" / "python-workflow-discovery-v1"
        qrels = build_synthetic_qrels.build_qrels(fixture)
        expected = (fixture / "qrels.json").read_text(encoding="utf-8")
        actual = json.dumps(qrels, indent=2) + "\n"
        self.assertEqual(actual, expected)

    def test_reproduces_rust_lane_qrels_byte_for_byte(self) -> None:
        fixture = LANES / "fixtures" / "rust-workflow-discovery-v1"
        qrels = build_synthetic_qrels.build_qrels(fixture)
        expected = (fixture / "qrels.json").read_text(encoding="utf-8")
        actual = json.dumps(qrels, indent=2) + "\n"
        self.assertEqual(actual, expected)

    def test_reproduces_typescript_lane_qrels_byte_for_byte(self) -> None:
        fixture = LANES / "fixtures" / "typescript-workflow-discovery-v1"
        qrels = build_synthetic_qrels.build_qrels(fixture)
        expected = (fixture / "qrels.json").read_text(encoding="utf-8")
        actual = json.dumps(qrels, indent=2) + "\n"
        self.assertEqual(actual, expected)

    def test_java_lane_snapshot_is_known_stale(self) -> None:
        # The committed java qrels snapshot does not match the current java
        # sources under the builder's convention (same 9 files and byte
        # total, different digest) — a pre-existing inconsistency recorded
        # in the spike report, deliberately not "fixed" here because the
        # lane's files are hash-pinned. This test pins the known state so a
        # future regeneration that resolves it is a visible event.
        fixture = LANES / "fixtures" / "java-workflow-discovery-v1"
        qrels = build_synthetic_qrels.build_qrels(fixture)
        committed = json.loads(
            (fixture / "qrels.json").read_text(encoding="utf-8")
        )
        self.assertNotEqual(
            qrels["source"]["snapshot_sha256"],
            committed["source"]["snapshot_sha256"],
        )

    def test_rejects_unknown_target_unit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "manifest.json").write_text(json.dumps({
                "fixture_id": "x-v1",
                "source": {"include": ["**/*.py"], "exclude": []},
                "units": [{"unit_id": "a.py::A", "path": "a.py",
                           "symbol": "A", "start_line": 1, "end_line": 1}],
            }))
            (root / "truth.json").write_text(json.dumps({
                "suite_id": "s",
                "queries": [{"id": "q", "query": "where",
                             "category": "c",
                             "judgments": [{"unit_id": "a.py::B", "grade": 3,
                                            "rationale": "x"}]}],
            }))
            with self.assertRaises(build_synthetic_qrels.FixtureError):
                build_synthetic_qrels.build_qrels(root)


if __name__ == "__main__":
    unittest.main()
