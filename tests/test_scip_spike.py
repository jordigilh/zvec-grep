import pathlib
import tempfile
import unittest

from scripts.scip_spike import attest, digest, join_snapshot, position, range_bytes, snapshot_key


class FakeRange:
    def __init__(self, **values):
        self.__dict__.update(values)


class FakeOccurrence:
    def __init__(self, variant=None, enclosing=None, legacy=()):
        self.variant = variant
        self.enclosing = enclosing
        self.range = legacy
        self.enclosing_range = ()
        if variant:
            setattr(self, variant[0], variant[1])
        if enclosing:
            setattr(self, enclosing[0], enclosing[1])

    def WhichOneof(self, name):
        return ((self.variant if name == 'typed_range' else self.enclosing) or (None,))[0]


class RangeTests(unittest.TestCase):
    def test_unicode_crlf_all_encodings_and_legacy(self):
        text = 'é🚀 X\r\nZ\n'
        self.assertEqual([position(text, 0, c, 1) for c in (2, 6, 7)], [2, 6, 7])
        self.assertEqual([position(text, 0, c, 2) for c in (1, 3, 4)], [2, 6, 7])
        self.assertEqual([position(text, 0, c, 3) for c in (1, 2, 3)], [2, 6, 7])
        self.assertEqual(position(text, 1, 0, 1), 10)
        self.assertEqual(position(text, 2, 0, 1), len(text.encode()))
        for encoding, start, end in ((1, 2, 6), (2, 1, 3), (3, 1, 2)):
            typed = FakeOccurrence(('single_line_range', FakeRange(line=0, start_character=start, end_character=end)))
            legacy = FakeOccurrence(legacy=(0, start, end))
            multiline = FakeOccurrence(('multi_line_range', FakeRange(start_line=0, start_character=start, end_line=1, end_character=1)))
            self.assertEqual(range_bytes(text, typed, encoding), (2, 6))
            self.assertEqual(range_bytes(text, legacy, encoding), (2, 6))
            self.assertEqual(range_bytes(text, multiline, encoding), (2, 11))
        self.assertEqual(range_bytes(text, FakeOccurrence(legacy=(0, 2, 1, 1)), 1), (2, 11))

    def test_reject_invalid_positions_and_prefer_typed(self):
        text = 'é🚀\r\n'
        for encoding, col in ((0, 0), (1, 1), (2, 2), (3, 3), (1, 7)):
            with self.subTest(encoding=encoding, col=col), self.assertRaises(ValueError):
                position(text, 0, col, encoding)
        with self.assertRaises(ValueError):
            range_bytes(text, FakeOccurrence(legacy=(0, 0, 3, 9)), 1)
        occ = FakeOccurrence(('single_line_range', FakeRange(line=0, start_character=2, end_character=6)), legacy=(99, 0, 0))
        self.assertEqual(range_bytes(text, occ, 1), (2, 6))


class SnapshotTests(unittest.TestCase):
    def test_full_set_fresh_dirty_delete_rename_cross_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'reference.ts').write_text('import {target} from "./target";\n')
            (root / 'target.ts').write_text('export const target = 1;\n')
            paths = ['reference.ts', 'target.ts']
            key = snapshot_key(root, paths, 'scip-typescript', '0.4.0', 'config-hash', 'dirty:one')
            self.assertEqual(key, snapshot_key(root, paths, 'scip-typescript', '0.4.0', 'config-hash', 'dirty:one'))
            self.assertTrue(attest(root, paths, digest(root, paths)))
            self.assertTrue(join_snapshot(key, key))
            (root / 'target.ts').write_text('export const target = "changed";\n')
            with self.assertRaises(ValueError):
                join_snapshot(key, snapshot_key(root, paths, 'scip-typescript', '0.4.0', 'config-hash', 'dirty:one'))
            with self.assertRaises(ValueError):
                attest(root, paths, 'stale')
            fresh_dirty = snapshot_key(root, paths, 'scip-typescript', '0.4.0', 'config-hash', 'dirty:two')
            self.assertTrue(join_snapshot(fresh_dirty, fresh_dirty))
            (root / 'target.ts').rename(root / 'renamed.ts')
            with self.assertRaises(FileNotFoundError):
                digest(root, paths)
            renamed = snapshot_key(root, ['reference.ts', 'renamed.ts'], 'scip-typescript', '0.4.0', 'config-hash', 'dirty:three')
            with self.assertRaises(ValueError):
                join_snapshot(fresh_dirty, renamed)
            (root / 'renamed.ts').unlink()
            with self.assertRaises(FileNotFoundError):
                digest(root, ['reference.ts', 'renamed.ts'])
            with self.assertRaises(ValueError):
                join_snapshot(None, renamed)


if __name__ == '__main__':
    unittest.main()
