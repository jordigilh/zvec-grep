"""Offline provenance and failure gates for the controlled, opt-in runner."""
import argparse
import importlib.util
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "code_ir_controlled_runner", REPO / "scripts/replay_code_ir_controlled.py"
)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)
TEMP = Path("/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode")


class ControlledRunnerTest(unittest.TestCase):
    def test_v2_uses_only_three_safe_source_files_of_the_expected_language(self):
        truth = {"language": "go", "schema_version": 2, "fixture_id": "controlled-go-v2",
                 "files": {"core.go": "a", "decoy.go": "b", "lures.go": "c"}}
        self.assertEqual(runner.fixture_paths(truth), ("core.go", "decoy.go", "lures.go"))
        for bad in (
            {"core.go": "a", "decoy.go": "b", "../escape.go": "c"},
            {"core.go": "a", "decoy.go": "b", "lures.ts": "c"},
            {"core.go": "a", "decoy.go": "b"},
        ):
            with self.subTest(paths=bad), self.assertRaises(ValueError):
                runner.fixture_paths({**truth, "files": bad})

    def test_model_cache_digest_is_content_and_path_pinned_and_refuses_symlinks(self):
        with tempfile.TemporaryDirectory(dir=TEMP) as directory:
            root = Path(directory)
            (root / "sub").mkdir()
            (root / "sub" / "weights").write_bytes(b"first")
            initial = runner.tree_digest(root)
            self.assertEqual(initial, runner.tree_digest(root))
            (root / "sub" / "weights").write_bytes(b"other")
            self.assertNotEqual(initial, runner.tree_digest(root))
            (root / "alias").symlink_to(root / "sub" / "weights")
            with self.assertRaisesRegex(ValueError, "symlink"):
                runner.tree_digest(root)

    def test_refuses_stale_source_and_checkout_outputs_before_staging(self):
        with tempfile.TemporaryDirectory(dir=TEMP) as directory:
            parent = Path(directory)
            fixture = parent / "go"
            fixture.mkdir()
            source = REPO / "test/fixtures/code-ir-controlled-v1/go"
            for path in source.iterdir():
                (fixture / path.name).write_bytes(path.read_bytes())
            cache = parent / "cache"
            cache.mkdir()
            options = argparse.Namespace(
                fixture=fixture, model_cache=cache,
                work_dir=parent / "work", output_dir=parent / "results",
                model="local/potion-code-16m-v2",
            )
            (fixture / "core.go").write_bytes(b"stale")
            with self.assertRaisesRegex(ValueError, "stale source"):
                runner.run(options)
            self.assertFalse(options.work_dir.exists())
            options.work_dir = source / "wrong-work"
            with self.assertRaisesRegex(ValueError, "outside the checkout"):
                runner.run(options)


if __name__ == "__main__":
    unittest.main()
