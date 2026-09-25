import unittest
from types import SimpleNamespace

from scripts.scip_compat_reassessment import position_rule


class VersionPinnedScipEncodingTests(unittest.TestCase):
    def test_control_supported_legacy_versions_only(self):
        docs = [SimpleNamespace(position_encoding=0)]
        self.assertEqual(position_rule('typescript', 'scip-typescript', '0.4.0', docs),
                         (2, 'utf-16', 'pinned-producer-unicode-control'))
        self.assertEqual(position_rule('python', 'scip-python', '0.6.6', docs)[0], 2)
        self.assertEqual(position_rule('go', 'scip-go', '0.2.7', docs)[0], 1)
        with self.assertRaisesRegex(ValueError, 'unknown SCIP producer/version'):
            position_rule('go', 'scip-go', '0.2.8', docs)

    def test_declared_encoding_is_accepted_only_if_consistent_with_pinned_contract(self):
        self.assertEqual(position_rule('python', 'scip-python', '0.6.6',
                                       [SimpleNamespace(position_encoding=2)])[2], 'producer-declared')
        for positions in ((0, 2), (3,), (0, 1)):
            with self.subTest(positions=positions), self.assertRaisesRegex(ValueError, 'mixed/conflicting'):
                position_rule('typescript', 'scip-typescript', '0.4.0',
                              [SimpleNamespace(position_encoding=p) for p in positions])


if __name__ == '__main__':
    unittest.main()
