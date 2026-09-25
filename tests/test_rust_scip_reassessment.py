import hashlib
import pathlib
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from scripts import rust_scip_reassessment as audit
from scripts import rust_scip_shadow_import as shadow
from scripts.scip_spike import digest


class RustScipScopeTests(unittest.TestCase):
    def test_scip_byte_token_evidence_on_original_crlf_and_astral(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'src'
            source.mkdir()
            data = 'const MSG: &str = "é🚀"; fn target() {}\r\n'.encode()
            (source / 'lib.rs').write_bytes(data)
            a = data.index(b'target()')
            evidence = shadow.source_evidence(root, 'src/lib.rs', a, a + 6)
            self.assertEqual((evidence['slice'], evidence['start_byte'], evidence['byte_column']),
                             ('target', a, a))
            with self.assertRaises(ValueError):
                shadow.source_evidence(root, 'src/lib.rs', 20, 21)  # continuation byte

    def test_local_ids_are_scoped_to_document(self):
        self.assertNotEqual(audit.symbol_key('local 0', 'a.rs'), audit.symbol_key('local 0', 'b.rs'))
        self.assertEqual(audit.symbol_key('rust-analyzer cargo demo 1.0 Type#', 'a.rs'),
                         audit.symbol_key('rust-analyzer cargo demo 1.0 Type#', 'b.rs'))

    def test_full_set_attestation_and_unknown_encoding_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary).resolve()
            lane = root / 'rust'
            source = lane / 'src'
            source.mkdir(parents=True)
            (source / 'lib.rs').write_bytes(b'pub struct Foo;\n')
            (root / 'scip.proto').write_bytes(b'fake pinned proto')
            paths = ['src/lib.rs']
            before = digest(lane, paths)
            document = SimpleNamespace(relative_path=paths[0], position_encoding=1)
            index = SimpleNamespace(metadata=SimpleNamespace(project_root=lane.as_uri()), documents=[document])
            ir = {'provenance': {'staged_root': str(lane)}, 'snapshot': {'files': [
                {'relative_path': paths[0], 'sha256': hashlib.sha256((source / 'lib.rs').read_bytes()).hexdigest()}]}}
            lsp = {'provenance': {'staged_root': str(lane), 'selected_paths': paths}}
            with (patch.object(audit, 'selected', return_value=(None, None, paths)),
                  patch.dict(audit.SOURCE_DIGESTS, {'rust': before}),
                  patch.object(audit, 'PINNED_PROTO_SHA256', hashlib.sha256(b'fake pinned proto').hexdigest())):
                self.assertEqual(audit.verify_inputs(root, index, ir, lsp)[1], paths)
                document.position_encoding = 0
                with self.assertRaisesRegex(ValueError, 'unknown position encoding'):
                    audit.verify_inputs(root, index, ir, lsp)
                document.position_encoding = 1
                (source / 'lib.rs').write_bytes(b'pub struct Bar;\n')
                with self.assertRaisesRegex(ValueError, 'stale full selected source set'):
                    audit.verify_inputs(root, index, ir, lsp)


if __name__ == '__main__':
    unittest.main()
