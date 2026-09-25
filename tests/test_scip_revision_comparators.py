import json
import tempfile
import unittest
from pathlib import Path

from scripts.compare_shadow_factsets import compare
from scripts.compare_scip_go_revisions import definitions, primary_digest


class FakeOccurrence:
    def __init__(self, symbol, roles=1, enclosing=()):
        self.symbol = symbol
        self.symbol_roles = roles
        self.range = [0, 4, 4 + len(symbol)]
        self.enclosing_range = enclosing

    def WhichOneof(self, _):
        return None


class FakeDocument:
    def __init__(self, occurrences):
        self.occurrences = occurrences


class ScipRevisionComparatorTests(unittest.TestCase):
    def test_primary_sites_are_order_independent_but_role_and_symbol_sensitive(self):
        first = FakeOccurrence("same", roles=1, enclosing=[0, 0, 10])
        second = FakeOccurrence("same", roles=0)
        ordered = FakeDocument([first, second])
        reversed_order = FakeDocument([second, first])
        self.assertEqual(primary_digest(ordered), primary_digest(reversed_order))
        self.assertNotEqual(primary_digest(ordered)[0], primary_digest(FakeDocument([first, first]))[0])
        # New declaration bounds can be added without altering navigation sites.
        first.enclosing_range = []
        self.assertEqual(primary_digest(ordered)[0], primary_digest(reversed_order)[0])
        self.assertEqual(definitions(ordered)[(0, 4, 0, 8, "same", 1)][None], 1)

    def test_equal_fact_counts_do_not_hide_changed_relationship_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            old_path = Path(directory) / "old.json"
            new_path = Path(directory) / "new.json"
            source = {"schema": "zvec-grep.code-ir", "snapshot_id": "snapshot",
                      "units": [{"id": "left"}, {"id": "right"}],
                      "facts": [{"id": "ref", "kind": "references", "object_id": "left"}]}
            old_path.write_text(json.dumps({"snapshot": source}))
            changed = {**source, "facts": [{"id": "ref", "kind": "references", "object_id": "right"}]}
            new_path.write_text(json.dumps({"snapshot": changed}))
            result = compare(old_path, new_path)
            self.assertEqual((result["old_facts"], result["new_facts"]), (1, 1))
            self.assertEqual(result["changed"], 1)
            self.assertFalse(result["equal"])
            changed["snapshot_id"] = "other"
            new_path.write_text(json.dumps({"snapshot": changed}))
            with self.assertRaisesRegex(ValueError, "different source/frontends"):
                compare(old_path, new_path)


if __name__ == "__main__":
    unittest.main()
