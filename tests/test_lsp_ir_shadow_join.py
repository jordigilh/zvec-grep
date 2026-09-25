import hashlib
import pathlib
import tempfile
import unittest
from unittest.mock import patch

from scripts import lsp_ir_shadow_join as join
from scripts.scip_spike import digest, snapshot_key


class StrictShadowJoinTests(unittest.TestCase):
    def test_body_and_kind_mismatch_abstain_even_with_same_name_and_token(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            raw = b'export function add() {}\n'
            (root / 'code.ts').write_bytes(raw)
            sha = hashlib.sha256(raw).hexdigest()
            files = {'code.ts': {'file_id': 'f', 'sha256': sha}}
            start = raw.index(b'add')
            token = {'path': 'code.ts', 'start_byte': start, 'end_byte': start + 3,
                     'line': 1, 'byte_column': start, 'slice': 'add', 'source_sha256': sha}
            unit = {'id': 'u', 'name': 'add', 'kind': 'function',
                    'source': {'file_id': 'f', 'start_byte': 7, 'end_byte': len(raw) - 1}}
            symbol = {'name': 'add', 'kind': 12, 'token': token,
                      'body': {'start_byte': 0, 'end_byte': len(raw) - 1}}
            self.assertEqual(join.strict_symbol_binding(symbol, [unit], root, files)[1], 'body_or_kind_mismatch')
            unit['source']['start_byte'] = 0
            self.assertEqual(join.strict_symbol_binding(symbol, [unit], root, files)[1], 'strict')
            unit['kind'] = 'type'
            self.assertEqual(join.strict_symbol_binding(symbol, [unit], root, files)[1], 'body_or_kind_mismatch')

    def test_smallest_subject_unique_and_ambiguous_equal_spans(self):
        source = {'file_id': 'f', 'start_byte': 15, 'end_byte': 18}
        file = {'id': 'file', 'kind': 'file', 'source': {'file_id': 'f', 'start_byte': 0, 'end_byte': 50}}
        method = {'id': 'method', 'kind': 'method', 'source': {'file_id': 'f', 'start_byte': 10, 'end_byte': 30}}
        self.assertEqual(join.smallest_subject(source, [file, method])[0]['id'], 'method')
        self.assertEqual(join.smallest_subject(source, [file, {**method, 'id': 'a'}, method])[1], 'ambiguous_subject')
        self.assertEqual(join.smallest_subject({'file_id': 'other', 'start_byte': 15, 'end_byte': 18}, [file])[1], 'uncontained')

    def test_whole_selected_set_and_target_changes_reject_even_if_caller_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            lane = base / 'typescript'
            lane.mkdir()
            lane = lane.resolve(strict=True)
            (lane / 'caller.ts').write_bytes(b'run();\n')
            (lane / 'target.ts').write_bytes(b'function run() {}\n')
            (lane / 'tsconfig.json').write_bytes(b'{}')
            paths = ['caller.ts', 'target.ts']
            before = digest(lane, paths)
            config = hashlib.sha256(b'{}').hexdigest()
            key = snapshot_key(lane, paths, 'lsp', 'test-v1', config,
                f'Engram fixture-v1 staged copy; root={lane}; source-set={before}')
            root_id = 'typescript-workflow-discovery-v1'
            ir = {'provenance': {'staged_root': str(lane)}, 'snapshot': {
                'schema': 'zvec-grep.code-ir', 'schema_version': 1,
                'root_set': [root_id], 'files': [
                    {'file_id': name, 'relative_path': name, 'language': 'typescript',
                     'sha256': hashlib.sha256((lane / name).read_bytes()).hexdigest(),
                     'byte_length': (lane / name).stat().st_size, 'extraction': {'status': 'complete'}}
                    for name in paths]}}
            lsp = {'provenance': {'staged_root': str(lane), 'selected_paths': paths,
                'source_set_sha256': before, 'config_sha256': config, 'snapshot_key': key,
                'producer': 'test-v1', 'lsp_position_encoding': 'utf-16'}}
            with patch.object(join, 'selected', return_value=(None, None, paths)), patch.dict(join.SOURCE_DIGESTS, {'typescript': before}):
                join.inspect_snapshot(lane, 'typescript', lsp, ir)
                (lane / 'target.ts').write_bytes(b'function run() { return 2; }\n')
                with self.assertRaisesRegex(ValueError, 'stale selected source'):
                    join.inspect_snapshot(lane, 'typescript', lsp, ir)
                self.assertEqual((lane / 'caller.ts').read_bytes(), b'run();\n')
                (lane / 'target.ts').unlink()
                with self.assertRaises(FileNotFoundError):
                    join.inspect_snapshot(lane, 'typescript', lsp, ir)


if __name__ == '__main__':
    unittest.main()
