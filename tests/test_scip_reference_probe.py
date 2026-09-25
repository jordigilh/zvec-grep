import json
import pathlib
import tempfile
import unittest

from scripts.scip_reference_probe import LABELS, SOURCE_DIGESTS, anchor, location_matches
from scripts.scip_spike import selected, digest


class AuthoredInventoryTests(unittest.TestCase):
    def test_all_sites_and_definitions_exist_on_original_frozen_bytes(self):
        inventory = json.loads(LABELS.read_text())
        for lang, expected in SOURCE_DIGESTS.items():
            root, manifest, paths = selected(lang)
            self.assertEqual(digest(root, paths), expected, lang)
            self.assertEqual(len(inventory[lang]), 5)
            for row in inventory[lang]:
                with self.subTest(language=lang, site=row['site']):
                    self.assertIn(row['site'][0], paths)
                    self.assertIn(row['definition'][0], paths)
                    self.assertEqual(anchor(root, row['site'])['token'], row['site'][2])
                    self.assertEqual(anchor(root, row['definition'])['token'], row['definition'][2])

    def test_navigation_rejects_wrong_target_even_when_name_matches(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            expect = {'path': 'target.py', 'line': 2, 'start': 4, 'end': 12}
            wrong = {'uri': (root / 'other.py').as_uri(), 'range': {'start': {'line': 2, 'character': 4}}}
            self.assertFalse(location_matches([wrong], expect, root)[0])
            correct = {'uri': (root / 'target.py').as_uri(), 'range': {'start': {'line': 2, 'character': 4}}}
            self.assertTrue(location_matches([correct], expect, root)[0])


if __name__ == '__main__':
    unittest.main()
