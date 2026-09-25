"""Rust-only snapshot-attested SCIP -> strict Code IR shadow import.

python3 scripts/rust_scip_shadow_import.py ROOT
The intermediate structure mirrors the existing shadow join's token/body
shape, but its provenance and range encoding come solely from SCIP.
"""
import hashlib
import json
import pathlib
import sys

if __package__:
    from scripts.scip_spike import digest, range_bytes, selected, snapshot_key
    from scripts.rust_scip_reassessment import symbol_key, verify_inputs
    from scripts.lsp_ir_shadow_join import DESIGN_SCHEMA, shadow_join
else:
    from scip_spike import digest, range_bytes, selected, snapshot_key
    from rust_scip_reassessment import symbol_key, verify_inputs
    from lsp_ir_shadow_join import DESIGN_SCHEMA, shadow_join


def source_evidence(root, path, a, b):
    raw = (root / path).read_bytes()
    if not 0 <= a < b <= len(raw):
        raise ValueError('invalid SCIP evidence')
    token = raw[a:b].decode('utf-8')
    prefix = raw[:a]
    return {'path': path, 'start_byte': a, 'end_byte': b, 'line': prefix.count(b'\n') + 1,
            'byte_column': len(prefix) - (prefix.rfind(b'\n') + 1),
            'slice': token, 'source_sha256': hashlib.sha256(raw).hexdigest()}


def normalize(root, index):
    sys.path.insert(0, str(root))
    import scip_pb2
    lane = (root / 'rust').resolve(strict=True)
    paths = selected('rust')[2]
    config_sha = hashlib.sha256((lane / 'Cargo.toml').read_bytes()).hexdigest()
    files_sha = digest(lane, paths)
    tool = index.metadata.tool_info
    version = f'{tool.name}@{tool.version}'
    kind_map = {
        scip_pb2.SymbolInformation.Kind.Function: 12,
        scip_pb2.SymbolInformation.Kind.Method: 6,
        scip_pb2.SymbolInformation.Kind.StaticMethod: 6,
        scip_pb2.SymbolInformation.Kind.Struct: 23,
        scip_pb2.SymbolInformation.Kind.Class: 5,
        scip_pb2.SymbolInformation.Kind.Enum: 10,
        scip_pb2.SymbolInformation.Kind.Trait: 11,
        scip_pb2.SymbolInformation.Kind.Module: 2,
        scip_pb2.SymbolInformation.Kind.Field: 8,
        scip_pb2.SymbolInformation.Kind.Variable: 13,
        scip_pb2.SymbolInformation.Kind.Parameter: 13,
    }
    infos = {}
    for doc in index.documents:
        for info in doc.symbols:
            infos[symbol_key(info.symbol, doc.relative_path)] = info
    definitions = {}
    candidates = {}
    for doc in index.documents:
        text = (lane / doc.relative_path).read_bytes().decode('utf-8')
        for occurrence in doc.occurrences:
            if not occurrence.symbol or not occurrence.symbol_roles & 1:
                continue
            key = symbol_key(occurrence.symbol, doc.relative_path)
            a, b = range_bytes(text, occurrence, doc.position_encoding)
            if not (occurrence.enclosing_range or occurrence.WhichOneof('typed_enclosing_range')):
                continue
            x, y = range_bytes(text, occurrence, doc.position_encoding, enclosing=True)
            if not x <= a < b <= y:
                continue
            info = infos.get(key)
            if not info or info.kind not in kind_map:
                continue
            candidate = {'name': info.display_name or source_evidence(lane, doc.relative_path, a, b)['slice'],
                         'kind': kind_map[info.kind],
                         'token': source_evidence(lane, doc.relative_path, a, b),
                         'body': {'start_byte': x, 'end_byte': y}}
            candidates.setdefault(key, []).append(candidate)
    for key, sites in candidates.items():
        if len(sites) == 1:
            definitions[key] = sites[0]
    references = {}
    dropped = {'no_local_definition': 0, 'nonunique_or_missing_enclosing_definition': 0}
    for doc in index.documents:
        text = (lane / doc.relative_path).read_bytes().decode('utf-8')
        for occurrence in doc.occurrences:
            if not occurrence.symbol or occurrence.symbol_roles & 1:
                continue
            key = symbol_key(occurrence.symbol, doc.relative_path)
            if key not in definitions:
                reason = 'nonunique_or_missing_enclosing_definition' if key in candidates else 'no_local_definition'
                dropped[reason] += 1
                continue
            a, b = range_bytes(text, occurrence, doc.position_encoding)
            site = source_evidence(lane, doc.relative_path, a, b)
            target = definitions[key]['token']
            identity = (site['path'], a, b, target['path'], target['start_byte'], target['end_byte'])
            references[identity] = {'site': site, 'target': target, 'relation': 'references',
                                     'method': 'SCIP symbol occurrence', 'resolution': 'semantic_candidate'}
    key = snapshot_key(lane, paths, 'scip', version, config_sha,
                       f'Engram fixture-v1 staged copy; root={lane}; source-set={files_sha}')
    return {'schema': 'rust-scip-normalized-shadow-v1', 'language': 'rust',
        'provenance': {'semantic_source': 'scip', 'selected_paths': paths,
                       'staged_root': str(lane), 'source_set_sha256': files_sha,
                       'config_sha256': config_sha, 'snapshot_key': key,
                       'producer': version, 'lsp_position_encoding': 'utf-8'},
        'symbols': list(definitions.values()), 'references': list(references.values()),
        'dropped_reference_sites': dropped}


def run(root):
    sys.path.insert(0, str(root))
    import scip_pb2
    raw = (root / 'rust/index.scip').read_bytes()
    index = scip_pb2.Index()
    index.ParseFromString(raw)
    ir_bytes = (root / 'syntax-ir-rust.json').read_bytes()
    ir = json.loads(ir_bytes)
    lane, _ = verify_inputs(root, index, ir)
    normalized = normalize(root, index)
    enriched, counts = shadow_join(lane, 'rust', normalized, ir)
    output = root / 'scip-shadow-ir-rust.json'
    payload = (json.dumps({'snapshot': enriched, 'join_counts': counts,
                           'scip_artifact_sha256': hashlib.sha256(raw).hexdigest(),
                           'syntax_artifact_sha256': hashlib.sha256(ir_bytes).hexdigest(),
                           'source': 'rust-analyzer native SCIP'}, sort_keys=True, separators=(',', ':')) + '\n').encode()
    with output.open('xb') as stream:
        stream.write(payload)
    print(json.dumps({'language': 'rust', 'scip_sha256': hashlib.sha256(raw).hexdigest(),
                      'normalized_symbols': len(normalized['symbols']),
                      'normalized_local_references': len(normalized['references']),
                      'excluded': normalized['dropped_reference_sites'], 'join': counts,
                      'artifact': str(output), 'artifact_sha256': hashlib.sha256(payload).hexdigest()}))


if __name__ == '__main__':
    run(pathlib.Path(sys.argv[1]).resolve(strict=True))
