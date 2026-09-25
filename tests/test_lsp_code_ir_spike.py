import hashlib
import pathlib
import tempfile
import unittest

from scripts.lsp_code_ir_spike import byte_span, eligible_reference_seed, evidence, exclude_conflicting_sites, negotiated_encoding, path_from_uri, verify_snapshot
from scripts.scip_spike import digest


class LspAdapterTests(unittest.TestCase):
    def test_lsp_omitted_encoding_is_utf16_and_unknown_is_rejected(self):
        self.assertEqual(negotiated_encoding({}), 'utf-16')
        self.assertEqual(negotiated_encoding({'positionEncoding': 'utf-8'}), 'utf-8')
        with self.assertRaises(ValueError):
            negotiated_encoding({'positionEncoding': 'other'})

    def test_utf16_lsp_span_to_original_utf8_bytes_crlf_and_astral(self):
        raw = '"é🚀" target\r\ntarget\r\n'.encode()
        first = {'start': {'line': 0, 'character': 6}, 'end': {'line': 0, 'character': 12}}
        second = {'start': {'line': 1, 'character': 0}, 'end': {'line': 1, 'character': 6}}
        self.assertEqual(byte_span(raw, first, 'utf-16'), [9, 15])
        self.assertEqual(byte_span(raw, second, 'utf-16'), [17, 23])
        with self.assertRaises(ValueError):
            byte_span(raw, {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 4}}, 'utf-16')
        with self.assertRaises(ValueError):
            byte_span(raw, first, 'unknown')
        with self.assertRaises(ValueError):
            byte_span(raw, {'start': second['end'], 'end': second['start']}, 'utf-16')

    def test_selected_root_and_slice_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'source.ts').write_bytes(b'const target = 1;\n')
            (root / 'other.ts').write_bytes(b'const target = 2;\n')
            allowed = ['source.ts']
            where = {'start': {'line': 0, 'character': 6}, 'end': {'line': 0, 'character': 12}}
            site = evidence(root, allowed, (root / 'source.ts').as_uri(), where, 'utf-16', 'target')
            self.assertEqual((site['start_byte'], site['end_byte'], site['byte_column']), (6, 12, 6))
            with self.assertRaises(ValueError):
                evidence(root, allowed, (root / 'source.ts').as_uri(), where, 'utf-16', 'wrong')
            with self.assertRaises(ValueError):
                path_from_uri(root, allowed, (root / 'other.ts').as_uri())
            with self.assertRaises(ValueError):
                path_from_uri(root, allowed, 'https://example.com/source.ts')

    def test_cross_file_dirty_change_and_config_invalidate_whole_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'caller.ts').write_bytes(b'target();\n')
            (root / 'target.ts').write_bytes(b'function target() {}\n')
            (root / 'tsconfig.json').write_bytes(b'{}')
            paths = ['caller.ts', 'target.ts']
            before = digest(root, paths)
            config = hashlib.sha256(b'{}').hexdigest()
            verify_snapshot(root, paths, before, 'tsconfig.json', config)
            (root / 'target.ts').write_bytes(b'function target() { return 2; }\n')
            with self.assertRaises(ValueError):
                verify_snapshot(root, paths, before, 'tsconfig.json', config)
            after = digest(root, paths)
            (root / 'tsconfig.json').write_bytes(b'{"strict":true}')
            with self.assertRaises(ValueError):
                verify_snapshot(root, paths, after, 'tsconfig.json', config)

    def test_conflicting_lsp_targets_abstain_without_dropping_other_sites(self):
        facts = {
            'one': {'site': {'path': 'ref.rs', 'start_byte': 8, 'end_byte': 12},
                    'target': {'path': 'a.rs', 'start_byte': 4, 'end_byte': 8}},
            'two': {'site': {'path': 'ref.rs', 'start_byte': 8, 'end_byte': 12},
                    'target': {'path': 'b.rs', 'start_byte': 4, 'end_byte': 8}},
            'three': {'site': {'path': 'ref.rs', 'start_byte': 30, 'end_byte': 34},
                      'target': {'path': 'a.rs', 'start_byte': 4, 'end_byte': 8}},
        }
        accepted, count, examples = exclude_conflicting_sites(facts)
        self.assertEqual((len(accepted), count), (1, 1))
        self.assertEqual(accepted[0], facts['three'])
        self.assertEqual(len(examples[0]['targets']), 2)

    def test_rust_impl_pseudounit_is_not_type_reference_target(self):
        self.assertFalse(eligible_reference_seed({'name': 'impl WorkflowState'}))
        self.assertTrue(eligible_reference_seed({'name': 'WorkflowState'}))
        self.assertTrue(eligible_reference_seed({'name': 'contains'}))


if __name__ == '__main__':
    unittest.main()
