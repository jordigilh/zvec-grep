"""Measure a pinned, fail-closed legacy SCIP encoding compatibility adapter.

python3 scripts/scip_compat_reassessment.py ROOT [typescript python go]

This never rewrites index.scip or changes production defaults. For exactly
known producer versions only, it applies empirically tested encoding rules
from separate astral/CRLF producer controls and emits a shadow-only IR join.
An upstream explicit Document.position_encoding remains the preferred fix.
"""
import collections
import hashlib
import json
import pathlib
import sys
import urllib.parse

if __package__:
    from scripts.scip_spike import digest, range_bytes, selected, snapshot_key
    from scripts.scip_reference_probe import LABELS, SOURCE_DIGESTS, anchor
    from scripts.lsp_ir_shadow_join import shadow_join, strict_symbol_binding, inspect_snapshot as inspect_ir_snapshot
    from scripts.rust_scip_reassessment import symbol_key
    from scripts.rust_scip_shadow_import import source_evidence
else:
    from scip_spike import digest, range_bytes, selected, snapshot_key
    from scip_reference_probe import LABELS, SOURCE_DIGESTS, anchor
    from lsp_ir_shadow_join import shadow_join, strict_symbol_binding, inspect_snapshot as inspect_ir_snapshot
    from rust_scip_reassessment import symbol_key
    from rust_scip_shadow_import import source_evidence

SCHEMA_SHA = 'b38021b65ef90cbbf6af9c829ff75192859ad9b5da05439ef154bea4ceb2bf03'
RULES = {
    'typescript': ('scip-typescript', '0.4.0', 2, 'utf-16', 'tsconfig.json'),
    'python': ('scip-python', '0.6.6', 2, 'utf-16', 'pyrightconfig.json'),
    'go': ('scip-go', '0.2.7', 1, 'utf-8', 'go.mod'),
}


def position_rule(language, tool_name, tool_version, documents):
    expected_name, expected_version, encoding, text_encoding, _ = RULES[language]
    if (tool_name, tool_version) != (expected_name, expected_version):
        raise ValueError('unknown SCIP producer/version; no encoding override')
    declared = {d.position_encoding for d in documents}
    if declared == {0}:
        return encoding, text_encoding, 'pinned-producer-unicode-control'
    if declared == {encoding}:
        return encoding, text_encoding, 'producer-declared'
    raise ValueError(f'mixed/conflicting SCIP position encodings {declared}')


def inspect_snapshot(root, language, index, ir_record):
    lane = (root / language).resolve(strict=True)
    if hashlib.sha256((root / 'scip.proto').read_bytes()).hexdigest() != SCHEMA_SHA:
        raise ValueError('SCIP protocol schema changed')
    paths = selected(language)[2]
    if digest(lane, paths) != SOURCE_DIGESTS[language]:
        raise ValueError('stale full selected source set')
    project = urllib.parse.urlparse(index.metadata.project_root)
    if project.scheme != 'file' or pathlib.Path(urllib.parse.unquote(project.path)).resolve(strict=True) != lane:
        raise ValueError('SCIP project root mismatch')
    if pathlib.Path(ir_record['provenance']['staged_root']).resolve(strict=True) != lane:
        raise ValueError('syntax IR project root mismatch')
    docs = {d.relative_path: d for d in index.documents}
    files = {f['relative_path']: f for f in ir_record['snapshot']['files']}
    if sorted(docs) != paths or sorted(files) != paths:
        raise ValueError('SCIP/IR selected source sets differ')
    for path in paths:
        source = (lane / path).read_bytes()
        if (lane / path).is_symlink() or files[path]['sha256'] != hashlib.sha256(source).hexdigest():
            raise ValueError('SCIP/IR original file bytes differ')
    rule = position_rule(language, index.metadata.tool_info.name, index.metadata.tool_info.version, index.documents)
    return lane, paths, rule


def kind_map(proto):
    kinds = proto.SymbolInformation.Kind
    return {kinds.Function: 12, kinds.Method: 6, kinds.StaticMethod: 6,
            kinds.Constructor: 9, kinds.Struct: 23, kinds.Class: 5,
            kinds.Enum: 10, kinds.Interface: 11, kinds.Trait: 11,
            kinds.Module: 2, kinds.Field: 8, kinds.Property: 7,
            kinds.Variable: 13, kinds.Parameter: 13, kinds.SelfParameter: 13,
            kinds.TypeAlias: 5}


def normalize(root, language, index, ir, proto):
    lane, paths, (encoding, text_encoding, method) = inspect_snapshot(root, language, index, ir)
    syntax_files = {f['relative_path']: f for f in ir['snapshot']['files']}
    syntax_units = ir['snapshot']['units']
    syntax_kinds = {'type': 5, 'method': 6, 'function': 12, 'value': 13, 'module': 2}
    infos = {}
    counts = collections.Counter()
    for doc in index.documents:
        for info in doc.symbols:
            infos[symbol_key(info.symbol, doc.relative_path)] = info
            counts['relationships'] += len(info.relationships)
            counts['implementation_relationships'] += sum(r.is_implementation for r in info.relationships)
    candidates = collections.defaultdict(list)
    for doc in index.documents:
        text = (lane / doc.relative_path).read_bytes().decode('utf-8')
        for occ in doc.occurrences:
            counts['occurrences'] += 1
            try:
                a, b = range_bytes(text, occ, encoding)
                if a >= b:
                    counts['empty_primary'] += 1
                    continue
            except ValueError:
                counts['invalid_primary'] += 1
                continue
            if occ.symbol_roles & 1:
                counts['definitions'] += 1
                if not occ.symbol:
                    continue
                key = symbol_key(occ.symbol, doc.relative_path)
                info = infos.get(key)
                if not (occ.enclosing_range or occ.WhichOneof('typed_enclosing_range')):
                    counts['definition_without_enclosing'] += 1
                    continue
                try:
                    x, y = range_bytes(text, occ, encoding, enclosing=True)
                    if not x <= a < b <= y:
                        raise ValueError('definition not contained by enclosing range')
                except ValueError:
                    counts['definition_invalid_enclosing'] += 1
                    continue
                counts['definition_valid_enclosing'] += 1
                token = source_evidence(lane, doc.relative_path, a, b)
                if info and info.kind in kind_map(proto):
                    kind = kind_map(proto)[info.kind]
                    counts['definition_explicit_kind'] += 1
                else:
                    # scip-typescript/scip-python leave fine-grained Kind=0.
                    # Infer ONLY from one syntax IR unit at the *exact same*
                    # source-backed declaration range and matching name/token.
                    matches = [unit for unit in syntax_units if unit['source']['file_id'] == syntax_files[doc.relative_path]['file_id']
                               and unit.get('name') == token['slice'] and unit['kind'] in syntax_kinds
                               and unit['source']['start_byte'] == x and unit['source']['end_byte'] == y
                               and x <= a < b <= y]
                    if len(matches) != 1:
                        counts['definition_without_supported_kind_or_exact_syntax'] += 1
                        continue
                    kind = syntax_kinds[matches[0]['kind']]
                    counts['definition_kind_from_exact_syntax'] += 1
                candidates[key].append({'name': (info.display_name if info and info.display_name else token['slice']),
                    'kind': kind, 'token': token,
                    'body': {'start_byte': x, 'end_byte': y}})
            elif occ.symbol:
                counts['references'] += 1
    definitions = {k: v[0] for k, v in candidates.items() if len(v) == 1}
    references = {}
    for doc in index.documents:
        text = (lane / doc.relative_path).read_bytes().decode('utf-8')
        for occ in doc.occurrences:
            if not occ.symbol or occ.symbol_roles & 1:
                continue
            key = symbol_key(occ.symbol, doc.relative_path)
            if key not in definitions:
                counts['reference_without_unique_local_definition'] += 1
                continue
            a, b = range_bytes(text, occ, encoding)
            site = source_evidence(lane, doc.relative_path, a, b)
            target = definitions[key]['token']
            identity = (site['path'], a, b, target['path'], target['start_byte'], target['end_byte'])
            references[identity] = {'site': site, 'target': target, 'relation': 'references',
                                    'method': method, 'resolution': 'semantic_candidate'}
    version = f'{index.metadata.tool_info.name}@{index.metadata.tool_info.version}'
    config_file = RULES[language][4]
    config_hash = hashlib.sha256((lane / config_file).read_bytes()).hexdigest()
    files_hash = digest(lane, paths)
    scope = snapshot_key(lane, paths, 'scip-compat', version, config_hash,
                         f'Engram fixture-v1 staged copy; root={lane}; source-set={files_hash}')
    normalized = {'schema': 'pinned-scip-compat-shadow-v1', 'language': language,
        'provenance': {'semantic_source': 'scip-compat', 'selected_paths': paths,
                       'staged_root': str(lane), 'source_set_sha256': files_hash,
                       'config_sha256': config_hash, 'snapshot_key': scope,
                       'producer': version, 'lsp_position_encoding': text_encoding,
                       'position_method': method},
        'symbols': list(definitions.values()), 'references': list(references.values())}
    return normalized, counts


def labeled_links(root, lang, record):
    lane = root / lang
    candidates = {(f['site']['path'], f['site']['start_byte']): (f['target']['path'], f['target']['start_byte'])
                  for f in record['references']}
    correct = 0
    for row in json.loads(LABELS.read_text())[lang]:
        site, target = anchor(lane, row['site']), anchor(lane, row['definition'])
        def absolute(where):
            lines = (lane / where['path']).read_bytes().splitlines(keepends=True)
            return sum(map(len, lines[:where['line']])) + where['start']
        correct += candidates.get((site['path'], absolute(site))) == (target['path'], absolute(target))
    return correct


def run(root, languages):
    sys.path.insert(0, str(root))
    import scip_pb2
    for lang in languages:
        source = (root / lang / 'index.scip').read_bytes()
        index = scip_pb2.Index()
        index.ParseFromString(source)
        syntax_bytes = (root / f'syntax-ir-{lang}.json').read_bytes()
        ir = json.loads(syntax_bytes)
        normalized, counts = normalize(root, lang, index, ir, scip_pb2)
        enriched, joined = shadow_join((root / lang).resolve(strict=True), lang, normalized, ir)
        output = root / f'compat-v2-shadow-ir-{lang}.json'
        payload = (json.dumps({'snapshot': enriched, 'join_counts': joined,
                               'scip_sha256': hashlib.sha256(source).hexdigest(),
                               'syntax_sha256': hashlib.sha256(syntax_bytes).hexdigest()},
                              sort_keys=True, separators=(',', ':')) + '\n').encode()
        with output.open('xb') as stream:
            stream.write(payload)
        print(json.dumps({'language': lang, 'tool': normalized['provenance']['producer'],
            'position_encoding': normalized['provenance']['lsp_position_encoding'],
            'position_method': normalized['provenance']['position_method'],
            'documents': len(index.documents), 'external_symbols': len(index.external_symbols),
            'raw': counts, 'normalized_symbols': len(normalized['symbols']),
            'normalized_local_references': len(normalized['references']),
            'labeled_navigation': [labeled_links(root, lang, normalized), 5],
            'join': joined, 'scip_sha256': hashlib.sha256(source).hexdigest(),
            'artifact': str(output), 'artifact_sha256': hashlib.sha256(payload).hexdigest()}), flush=True)


def inspect(root, language):
    sys.path.insert(0, str(root))
    import scip_pb2
    index = scip_pb2.Index()
    index.ParseFromString((root / language / 'index.scip').read_bytes())
    ir = json.loads((root / f'syntax-ir-{language}.json').read_text())
    normalized, _ = normalize(root, language, index, ir, scip_pb2)
    lane = (root / language).resolve(strict=True)
    snapshot, files = inspect_ir_snapshot(lane, language, normalized, ir)
    reasons = collections.Counter()
    prefixes = collections.Counter()
    examples = []
    for symbol in normalized['symbols']:
        _, status = strict_symbol_binding(symbol, snapshot['units'], lane, files)
        if status == 'strict':
            continue
        reasons[status] += 1
        token = symbol['token']
        alternatives = [unit for unit in snapshot['units'] if unit['source']['file_id'] == files[token['path']]['file_id']
                        and unit.get('name') == token['slice']]
        if status == 'body_or_kind_mismatch' and len(alternatives) == 1 and alternatives[0]['source']['end_byte'] == symbol['body']['end_byte']:
            prefixes[(lane / token['path']).read_bytes()[symbol['body']['start_byte']:alternatives[0]['source']['start_byte']].decode('utf-8', 'replace')] += 1
        if len(examples) < 12:
            examples.append({'status': status, 'path': token['path'], 'symbol': token['slice'],
                'scip_body': symbol['body'], 'kind': symbol['kind'],
                'syntax_units': [{'kind': unit['kind'], 'start': unit['source']['start_byte'],
                                  'end': unit['source']['end_byte']} for unit in alternatives[:2]],
                'leading_difference': [(lane / token['path']).read_bytes()[symbol['body']['start_byte']:unit['source']['start_byte']].decode('utf-8', 'replace')[:90]
                                      for unit in alternatives[:1]]})
    print(json.dumps({'language': language, 'reasons': reasons, 'same_end_leading_prefixes': prefixes,
                      'examples': examples}), flush=True)


if __name__ == '__main__':
    root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    if '--inspect' in sys.argv[2:]:
        inspect(root, sys.argv[3])
    else:
        run(root, tuple(sys.argv[2:]) or tuple(RULES))
