import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.scip_reference_probe import anchor


REPOSITORY = Path(__file__).resolve().parents[1]
ENGRAM_ROOT = Path(
    os.environ.get(
        "ENGRAM_ROOT",
        REPOSITORY.parent / "engram",
    )
).resolve()
FIXTURE = ENGRAM_ROOT / "benchmarks/semantic_search/fixtures/go-workflow-discovery-v1"


class GoSourceInventoryTests(unittest.TestCase):
    def test_go_ast_spans_cover_the_frozen_38_unit_manifest(self):
        if not FIXTURE.is_dir():
            self.skipTest("set ENGRAM_ROOT to the read-only Engram checkout")
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "inventory.json"
            subprocess.run(
                [
                    "go",
                    "run",
                    "scripts/go_ir_source_inventory.go",
                    "--fixture",
                    str(FIXTURE),
                    "--output",
                    str(output),
                ],
                cwd=REPOSITORY,
                check=True,
            )
            inventory = json.loads(output.read_text())

        manifest = json.loads((FIXTURE / "manifest.json").read_text())
        qrels = json.loads((FIXTURE / "qrels.json").read_text())
        self.assertEqual(inventory["source_set_sha256"], qrels["source"]["snapshot_sha256"])
        self.assertEqual(inventory["source_files"], 10)
        self.assertEqual(len(inventory["units"]), 38)
        self.assertEqual(
            {row["unit_id"] for row in inventory["units"]},
            {row["unit_id"] for row in manifest["units"]},
        )
        for unit in inventory["units"]:
            source = (FIXTURE / unit["path"]).read_bytes()
            name = unit["symbol"].split(".")[-1].encode()
            self.assertEqual(
                source[unit["identifier_start_byte"]:unit["identifier_end_byte"]],
                name,
                unit["unit_id"],
            )
            self.assertLess(unit["start_byte"], unit["end_byte"], unit["unit_id"])
            self.assertLessEqual(unit["start_byte"], unit["identifier_start_byte"])
            self.assertLessEqual(unit["identifier_end_byte"], unit["end_byte"])

    def test_expanded_reference_labels_are_source_anchored_and_unique(self):
        if not FIXTURE.is_dir():
            self.skipTest("set ENGRAM_ROOT to the read-only Engram checkout")
        labels = json.loads((REPOSITORY / "docs/go-reference-sites-v2.json").read_text())
        self.assertEqual(labels["source_set_sha256"], json.loads((FIXTURE / "qrels.json").read_text())["source"]["snapshot_sha256"])
        self.assertEqual(len(labels["go"]), 20)
        seen = set()
        for row in labels["go"]:
            site = tuple(row["site"])
            self.assertNotIn(site, seen)
            seen.add(site)
            anchor(FIXTURE, row["site"])
            anchor(FIXTURE, row["definition"])


if __name__ == "__main__":
    unittest.main()
