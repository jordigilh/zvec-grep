"""Compare real rust-analyzer SCIP against same-source Rust LSP and syntax Code IR.

python3 scripts/rust_scip_reassessment.py ROOT
No SCIP data is imported into default indexes; all inputs are isolated files.
"""
import collections
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.parse

if __package__:
    from scripts.scip_spike import digest, range_bytes, selected
    from scripts.scip_reference_probe import SOURCE_DIGESTS, LABELS, anchor
else:
    from scip_spike import digest, range_bytes, selected
    from scip_reference_probe import SOURCE_DIGESTS, LABELS, anchor

PINNED_PROTO_SHA256 = 'b38021b65ef90cbbf6af9c829ff75192859ad9b5da05439ef154bea4ceb2bf03'


def symbol_key(symbol, document):
    """SCIP `local X` is document-scoped, not an inter-file symbol."""
    return (document if symbol.startswith('local ') else '', symbol)


def verify_inputs(root, index, ir, lsp=None):
    lane = (root / 'rust').resolve(strict=True)
    paths = selected('rust')[2]
    if hashlib.sha256((root / 'scip.proto').read_bytes()).hexdigest() != PINNED_PROTO_SHA256:
        raise ValueError('SCIP protocol schema drift')
    if digest(lane, paths) != SOURCE_DIGESTS['rust'] or (lsp is not None and sorted(lsp['provenance']['selected_paths']) != paths):
        raise ValueError('stale full selected source set')
    parsed = urllib.parse.urlparse(index.metadata.project_root)
    if parsed.scheme != 'file' or pathlib.Path(urllib.parse.unquote(parsed.path)).resolve(strict=True) != lane:
        raise ValueError('SCIP project root does not equal attested source root')
    if pathlib.Path(ir['provenance']['staged_root']).resolve(strict=True) != lane:
        raise ValueError('syntax IR root does not equal SCIP root')
    if lsp is not None and pathlib.Path(lsp['provenance']['staged_root']).resolve(strict=True) != lane:
        raise ValueError('LSP root does not equal SCIP root')
    files = {f['relative_path']: f for f in ir['snapshot']['files']}
    documents = {d.relative_path: d for d in index.documents}
    if sorted(files) != paths or sorted(documents) != paths:
        raise ValueError('SCIP and IR file sets differ')
    for name in paths:
        raw = (lane / name).read_bytes()
        if files[name]['sha256'] != hashlib.sha256(raw).hexdigest():
            raise ValueError('stale syntax file digest')
        if documents[name].position_encoding not in (1, 2, 3):
            raise ValueError('SCIP unknown position encoding')
    return lane, paths


def check(root):
    sys.path.insert(0, str(root))
    import scip_pb2
    data = (root / 'rust/index.scip').read_bytes()
    index = scip_pb2.Index()
    index.ParseFromString(data)
    ir = json.loads((root / 'syntax-ir-rust.json').read_text())
    lsp = json.loads((root / 'lsp-full-rust.json').read_text())
    lane, paths = verify_inputs(root, index, ir, lsp)
    definitions = collections.defaultdict(list)
    infos = {}
    counts = collections.Counter()
    kind_histogram = collections.Counter()
    def_range_examples = []
    for doc in index.documents:
        raw = (lane / doc.relative_path).read_bytes()
        text = raw.decode('utf-8')
        for info in doc.symbols:
            infos[symbol_key(info.symbol, doc.relative_path)] = info
            counts['relationship_records'] += len(info.relationships)
        for occ in doc.occurrences:
            counts['occurrences'] += 1
            a, b = range_bytes(text, occ, doc.position_encoding)
            if not 0 <= a < b <= len(raw):
                raise ValueError('invalid SCIP occurrence source span')
            raw[a:b].decode('utf-8')
            if not occ.symbol:
                counts['no_symbol'] += 1
                continue
            key = symbol_key(occ.symbol, doc.relative_path)
            if occ.symbol_roles & 1:
                counts['definitions'] += 1
                kind_histogram[str(infos.get(key).kind if infos.get(key) else 'no_info')] += 1
                enclosing = None
                if occ.enclosing_range or occ.WhichOneof('typed_enclosing_range'):
                    counts['definition_enclosing_present'] += 1
                    try:
                        candidate = range_bytes(text, occ, doc.position_encoding, enclosing=True)
                        if candidate[0] <= a < b <= candidate[1]:
                            enclosing = candidate
                            counts['definition_enclosing_valid'] += 1
                        else:
                            counts['definition_enclosing_not_containing_token'] += 1
                    except ValueError:
                        counts['definition_enclosing_invalid_range'] += 1
                definitions[key].append((doc.relative_path, a, b, enclosing))
                if len(def_range_examples) < 4 and enclosing and b - a < 80:
                    def_range_examples.append({'path': doc.relative_path, 'token': raw[a:b].decode(),
                                               'token_bytes': [a, b], 'enclosing_bytes': enclosing})
            else:
                counts['references'] += 1
    units = ir['snapshot']['units']
    files = {f['relative_path']: f for f in ir['snapshot']['files']}
    # SCIP symbol metadata kind uses protobuf enumerations; the name/span/kind
    # gate is deliberately separate from LSP's full-body exactness gate.
    kinds = {scip_pb2.SymbolInformation.Kind.Function: 'function',
             scip_pb2.SymbolInformation.Kind.Method: 'method',
             scip_pb2.SymbolInformation.Kind.StaticMethod: 'method',
             scip_pb2.SymbolInformation.Kind.Class: 'type',
             scip_pb2.SymbolInformation.Kind.Struct: 'type',
             scip_pb2.SymbolInformation.Kind.Trait: 'type',
             scip_pb2.SymbolInformation.Kind.Interface: 'type',
             scip_pb2.SymbolInformation.Kind.Enum: 'type',
             scip_pb2.SymbolInformation.Kind.Module: 'module',
             scip_pb2.SymbolInformation.Kind.Field: 'value',
             scip_pb2.SymbolInformation.Kind.Variable: 'value'}
    bindings = {}
    enclosing_bindings = set()
    for key, sites in definitions.items():
        if len(sites) != 1:
            counts['nonunique_definition_symbols'] += 1
            continue
        path, a, b, enclosing = sites[0]
        info = infos.get(key)
        name = (lane / path).read_bytes()[a:b].decode('utf-8')
        matches = [unit for unit in units if unit['source']['file_id'] == files[path]['file_id']
                   and unit.get('name') == name and unit['kind'] == kinds.get(info.kind if info else -1)
                   and unit['source']['start_byte'] <= a < b <= unit['source']['end_byte']]
        if len(matches) == 1:
            counts['unique_definition_token_kind_unit_join'] += 1
            bindings[key] = matches[0]
            if enclosing == (matches[0]['source']['start_byte'], matches[0]['source']['end_byte']):
                counts['exact_enclosing_kind_unit_join'] += 1
                enclosing_bindings.add(key)
        elif matches:
            counts['ambiguous_definition_unit'] += 1
        else:
            counts['unjoined_definition_unit'] += 1
    lsp_sites = collections.defaultdict(set)
    for fact in lsp['references']:
        site = fact['site']
        target = fact['target']
        lsp_sites[(site['path'], site['start_byte'], site['end_byte'])].add(
            (target['path'], target['start_byte'], target['end_byte']))
    scip_sites = collections.defaultdict(set)
    comparison_examples = []
    disagreements_by_kind = collections.Counter()
    for doc in index.documents:
        raw = (lane / doc.relative_path).read_bytes()
        text = raw.decode('utf-8')
        for occ in doc.occurrences:
            if not occ.symbol or occ.symbol_roles & 1:
                continue
            a, b = range_bytes(text, occ, doc.position_encoding)
            key = symbol_key(occ.symbol, doc.relative_path)
            sites = definitions.get(key, [])
            if not sites:
                counts['reference_without_selected_definition'] += 1
                continue
            if len(sites) != 1:
                counts['reference_nonunique_definition'] += 1
                continue
            dpath, da, db, _ = sites[0]
            scip_sites[(doc.relative_path, a, b)].add((dpath, da, db))
            counts['reference_unique_local_target'] += 1
            if key in bindings:
                counts['reference_token_kind_unit_join'] += 1
                if key in enclosing_bindings:
                    counts['reference_exact_enclosing_kind_unit_join'] += 1
            else:
                counts['reference_local_target_without_IR_unit'] += 1
            targets = lsp_sites.get((doc.relative_path, a, b))
            if targets:
                counts['site_also_in_lsp'] += 1
                if (dpath, da, db) in targets:
                    counts['exact_target_agreement_with_lsp'] += 1
                else:
                    counts['different_target_from_lsp'] += 1
                    kind_name = scip_pb2.SymbolInformation.Kind.Name(infos[key].kind) if key in infos else 'no_info'
                    disagreements_by_kind[kind_name] += 1
                    if len(comparison_examples) < 5 or kind_name != 'Module':
                        comparison_examples.append({'path': doc.relative_path, 'site_bytes': [a, b],
                                                    'slice': raw[a:b].decode(), 'scip': [dpath, da, db],
                                                    'lsp': sorted(targets)[:3], 'scip_kind': kind_name})
    all_scip_sites = {(doc.relative_path, *range_bytes((lane / doc.relative_path).read_bytes().decode('utf-8'),
                      occ, doc.position_encoding)) for doc in index.documents for occ in doc.occurrences
                      if occ.symbol and not (occ.symbol_roles & 1)}
    counts['lsp_reference_sites_with_any_scip_occurrence'] = len(set(lsp_sites) & all_scip_sites)
    counts['lsp_reference_sites_with_unique_local_scip_target'] = len(set(lsp_sites) & set(scip_sites))
    label_rows = json.loads(LABELS.read_text())['rust']
    for row in label_rows:
        site = anchor(lane, row['site'])
        target = anchor(lane, row['definition'])
        def absolute(where):
            lines = (lane / where['path']).read_bytes().splitlines(keepends=True)
            return sum(map(len, lines[:where['line']])) + where['start']
        place = (site['path'], absolute(site), absolute(site) + len(site['token'].encode()))
        if (target['path'], absolute(target), absolute(target) + len(target['token'].encode())) in scip_sites[place]:
            counts['independent_labeled_navigation_match'] += 1
            key = next((symbol_key(occ.symbol, doc.relative_path) for doc in index.documents if doc.relative_path == site['path']
                        for occ in doc.occurrences if occ.symbol and not occ.symbol_roles & 1
                        and range_bytes((lane / doc.relative_path).read_bytes().decode('utf-8'), occ, doc.position_encoding) == place[1:]), None)
            if key in bindings:
                counts['independent_labeled_token_kind_unit_join'] += 1
                if key in enclosing_bindings:
                    counts['independent_labeled_exact_enclosing_unit_join'] += 1
    print(json.dumps({'language': 'rust', 'producer': index.metadata.tool_info.name,
        'producer_version': index.metadata.tool_info.version,
        'source_set_sha256': digest(lane, paths), 'scip_sha256': hashlib.sha256(data).hexdigest(),
        'bytes': len(data), 'documents': len(index.documents),
        'encodings': collections.Counter(d.position_encoding for d in index.documents),
        'kind_histogram_definitions': kind_histogram, 'counts': counts,
        'definition_examples': def_range_examples, 'disagreements_by_kind': disagreements_by_kind,
        'disagreements': comparison_examples}), flush=True)


def unicode_control(root):
    """Actual rust-analyzer SCIP output over astral UTF-8 and CRLF input."""
    sys.path.insert(0, str(root))
    import scip_pb2
    with tempfile.TemporaryDirectory(prefix='rust-scip-unicode-', dir=root) as directory:
        lane = pathlib.Path(directory)
        (lane / 'src').mkdir()
        (lane / 'Cargo.toml').write_bytes(b'[package]\nname = "scip-unicode-control"\nversion = "0.1.0"\nedition = "2021"\n')
        declaration = 'pub const MARKER: &str = "é🚀"; pub fn target() {}\r\n'.encode()
        referring = 'mod target;\r\npub fn caller() { let _ = "é🚀"; target::target(); }\r\n'.encode()
        (lane / 'src/target.rs').write_bytes(declaration)
        (lane / 'src/lib.rs').write_bytes(referring)
        started = time.monotonic()
        subprocess.run(['rust-analyzer', 'scip', '.', '--output', 'index.scip'], cwd=lane, check=True,
                       capture_output=True, text=True, timeout=90)
        index = scip_pb2.Index()
        scip_bytes = (lane / 'index.scip').read_bytes()
        index.ParseFromString(scip_bytes)
        docs = {d.relative_path: d for d in index.documents}
        if set(docs) != {'src/lib.rs', 'src/target.rs'}:
            raise ValueError(f'unexpected Rust SCIP documents: {list(docs)}')
        if any(d.position_encoding != 1 for d in docs.values()):
            raise ValueError('Rust SCIP failed to declare UTF-8 position encoding')
        tokens = []
        for path, raw in [('src/lib.rs', referring), ('src/target.rs', declaration)]:
            for occurrence in docs[path].occurrences:
                a, b = range_bytes(raw.decode('utf-8'), occurrence, docs[path].position_encoding)
                if raw[a:b] == b'target':
                    tokens.append((path, a, b, occurrence.symbol, bool(occurrence.symbol_roles & 1)))
        definitions = [row for row in tokens if row[0] == 'src/target.rs' and row[4]]
        usages = [row for row in tokens if row[0] == 'src/lib.rs' and not row[4]]
        expected_declaration = declaration.index(b'target()')
        expected_use = referring.index(b'target();')
        if (len(definitions) != 1 or definitions[0][1:3] != (expected_declaration, expected_declaration + 6)
            or not any(use[1:3] == (expected_use, expected_use + 6) and use[3] == definitions[0][3]
                       for use in usages)):
            raise ValueError(f'Rust SCIP unicode target/source round trip failed: {tokens}')
        print(json.dumps({'control': 'real rust-analyzer SCIP UTF-8 astral + CRLF',
                          'position_encoding': 1, 'documents': len(docs),
                          'source_set_sha256': digest(lane, ['src/lib.rs', 'src/target.rs']),
                          'scip_sha256': hashlib.sha256(scip_bytes).hexdigest(),
                          'declaration_bytes': [expected_declaration, expected_declaration + 6],
                          'reference_bytes': [expected_use, expected_use + 6],
                          'symbols_equal': True,
                          'elapsed_seconds': round(time.monotonic() - started, 2)}), flush=True)


if __name__ == '__main__':
    root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    if '--unicode-control' in sys.argv[2:]:
        unicode_control(root)
    else:
        check(root)
