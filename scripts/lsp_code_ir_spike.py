"""Experimental LSP -> attested UTF-8 byte facts, independent of SCIP ranges.

python3 scripts/lsp_code_ir_spike.py ROOT [typescript|python|rust|go ...]
python3 scripts/lsp_code_ir_spike.py ROOT --full
python3 scripts/lsp_code_ir_spike.py ROOT --unicode-control

--full does not read SCIP or the authored label inventory. It queries every
LSP document symbol and that symbol's references in each selected file.
Without --full, the authored reference inventory is a bounded evaluation.
"""
import hashlib
import json
import os
import pathlib
import sys
import tempfile
import time
import urllib.parse

if __package__:
    from scripts.scip_spike import LANGS, digest, position, selected, snapshot_key
    from scripts.scip_reference_probe import LABELS, SOURCE_DIGESTS, Lsp, anchor
else:
    from scip_spike import LANGS, digest, position, selected, snapshot_key
    from scip_reference_probe import LABELS, SOURCE_DIGESTS, Lsp, anchor

ENCODINGS = {'utf-8': 1, 'utf-16': 2, 'utf-32': 3}
VERSIONS = {'typescript': 'typescript-language-server@6.0.1/typescript@5.9.3',
            'python': 'pyright@1.1.414', 'rust': 'rust-analyzer@f938641be5', 'go': 'gopls@v0.23.0'}
CONFIG = {'typescript': 'tsconfig.json', 'python': 'pyrightconfig.json',
          'rust': 'Cargo.toml', 'go': 'go.mod'}


def negotiated_encoding(capabilities):
    # LSP 3.17: omitted positionEncoding is *specified* to mean UTF-16.
    encoding = capabilities.get('positionEncoding', 'utf-16')
    if encoding not in ENCODINGS:
        raise ValueError(f'unsupported LSP position encoding {encoding!r}')
    return encoding


def path_from_uri(root, allowed, uri):
    parsed = urllib.parse.urlparse(uri)
    if parsed.scheme != 'file' or parsed.netloc not in ('', 'localhost') or parsed.query or parsed.fragment:
        raise ValueError('non-file or non-local document URI')
    path = pathlib.Path(urllib.parse.unquote(parsed.path))
    canonical = path.resolve(strict=True)
    if path.is_symlink() or not canonical.is_file():
        raise ValueError('non-regular or symlink target')
    try:
        relative = canonical.relative_to(root.resolve(strict=True)).as_posix()
    except ValueError as exc:
        raise ValueError('cross-root document') from exc
    if relative not in allowed:
        raise ValueError('document outside selected source set')
    return relative


def byte_span(raw, location, encoding):
    if encoding not in ENCODINGS:
        raise ValueError('unverified LSP encoding')
    text = raw.decode('utf-8')
    a = location['start']
    b = location['end']
    start = position(text, a['line'], a['character'], ENCODINGS[encoding])
    end = position(text, b['line'], b['character'], ENCODINGS[encoding])
    if start >= end or end > len(raw):
        raise ValueError('empty/reversed/out-of-file source range')
    return [start, end]


def verify_snapshot(root, paths, expected_files, config_path, expected_config):
    if digest(root, paths) != expected_files or hashlib.sha256((root / config_path).read_bytes()).hexdigest() != expected_config:
        raise ValueError('source/config changed during LSP session; discard all semantic facts')


def evidence(root, selected_paths, uri, location, encoding, expected=None):
    relative = path_from_uri(root, selected_paths, uri)
    raw = (root / relative).read_bytes()
    span = byte_span(raw, location, encoding)
    token = raw[span[0]:span[1]].decode('utf-8')
    if expected is not None and token != expected:
        raise ValueError(f'LSP range slice {token!r} != authored token {expected!r}')
    last_newline = max(raw.rfind(b'\n', 0, span[0]), raw.rfind(b'\r', 0, span[0]))
    return {'path': relative, 'start_byte': span[0], 'end_byte': span[1],
            'line': location['start']['line'] + 1,
            'byte_column': span[0] - (last_newline + 1),
            'slice': token, 'source_sha256': hashlib.sha256(raw).hexdigest()}


def flatten_symbols(symbols, uri):
    for symbol in symbols or []:
        if 'selectionRange' in symbol:
            yield symbol['name'], symbol['kind'], uri, symbol['selectionRange'], symbol['range']
            yield from flatten_symbols(symbol.get('children', []), uri)
        elif 'location' in symbol:  # older SymbolInformation shape
            place = symbol['location']
            yield symbol['name'], symbol['kind'], place['uri'], place['range'], place['range']


def command_for(root, lang):
    tools = root / 'lsp-tools/node_modules'
    return {'typescript': ['node', str(tools / 'typescript-language-server/lib/cli.mjs'), '--stdio'],
            'python': ['node', str(tools / 'pyright/langserver.index.js'), '--stdio'],
            'rust': ['rust-analyzer'], 'go': ['gopls', 'serve']}[lang]


def init(server, root):
    uri = root.as_uri()
    response = server.request('initialize', {'processId': os.getpid(), 'rootPath': str(root), 'rootUri': uri,
        'workspaceFolders': [{'uri': uri, 'name': root.name}],
        'capabilities': {'general': {'positionEncodings': ['utf-16']}, 'textDocument': {
            'definition': {'linkSupport': True}, 'references': {}, 'documentSymbol': {'hierarchicalDocumentSymbolSupport': True}},
            'workspace': {'workspaceFolders': True}}}, 120)
    encoding = negotiated_encoding(response['capabilities'])
    server.send({'jsonrpc': '2.0', 'method': 'initialized', 'params': {}})
    return encoding, 'positionEncoding' not in response['capabilities']


def open_selected(server, root, paths, lang):
    for name in paths:
        server.send({'jsonrpc': '2.0', 'method': 'textDocument/didOpen', 'params': {'textDocument': {
            'uri': (root / name).as_uri(), 'languageId': lang, 'version': 1,
            'text': (root / name).read_bytes().decode('utf-8')}}})


def exclude_conflicting_sites(facts):
    """Do not assign a reference site to multiple distinct definitions."""
    targets = {}
    for fact in facts.values():
        site = fact['site']
        target = fact['target']
        key = (site['path'], site['start_byte'], site['end_byte'])
        targets.setdefault(key, set()).add((target['path'], target['start_byte'], target['end_byte']))
    conflicts = {key for key, links in targets.items() if len(links) > 1}
    accepted = [fact for fact in facts.values() if (fact['site']['path'], fact['site']['start_byte'], fact['site']['end_byte']) not in conflicts]
    examples = []
    for site in sorted(conflicts)[:5]:
        examples.append({'site': site, 'targets': sorted(targets[site])})
    return accepted, len(conflicts), examples


def eligible_reference_seed(symbol):
    # rust-analyzer exposes `impl Type` as a document symbol whose selection
    # is the Type token. Find-references at that token finds TYPE references;
    # assigning them to the impl pseudo-unit creates a false target.
    return not symbol['name'].startswith('impl ')


def publish_full(root, lang, record):
    """Publish a fresh generated artifact only after the input attestation passes."""
    output = root / f'lsp-full-{lang}.json'
    if output.exists():
        raise FileExistsError(f'generated artifact already exists: {output}; use a fresh temporary root')
    data = (json.dumps(record, sort_keys=True, separators=(',', ':')) + '\n').encode()
    with tempfile.NamedTemporaryFile(dir=root, prefix=f'.lsp-full-{lang}-', delete=False) as stream:
        try:
            stream.write(data)
            temporary = pathlib.Path(stream.name)
        except BaseException:
            pathlib.Path(stream.name).unlink(missing_ok=True)
            raise
    os.replace(temporary, output)
    return str(output), hashlib.sha256(data).hexdigest(), len(data)


def collect_full_references(server, lane, paths, encoding, symbols, symbol_positions):
    found = {}
    errors = []
    queries = 0
    for symbol in symbols:
        if not eligible_reference_seed(symbol):
            continue
        token = symbol['token']
        location_key = (token['path'], token['start_byte'], token['end_byte'], symbol['kind'])
        uri = (lane / token['path']).as_uri()
        queries += 1
        try:
            refs = server.request('textDocument/references', {'textDocument': {'uri': uri},
                'position': symbol_positions[location_key], 'context': {'includeDeclaration': False}}, 120)
            for item in refs or []:
                try:
                    source = evidence(lane, paths, item['uri'], item['range'], encoding)
                    if source['path'] == token['path'] and source['start_byte'] == token['start_byte']:
                        continue
                    key_ref = (source['path'], source['start_byte'], source['end_byte'],
                               token['path'], token['start_byte'], token['end_byte'])
                    found[key_ref] = {'relation': 'references', 'site': source,
                        'target': token, 'method': 'LSP find-references', 'resolution': 'semantic_candidate'}
                except (ValueError, KeyError) as exc:
                    errors.append({'symbol': token, 'reason': str(exc)})
        except (RuntimeError, TimeoutError) as exc:
            errors.append({'symbol': token, 'reason': str(exc)})
    return found, errors, queries


def run(root, languages, full=False):
    labels = None if full else json.loads(LABELS.read_text())
    for lang in languages:
        lane = (root / lang).resolve(strict=True)
        _, manifest, paths = selected(lang)
        before = digest(lane, paths)
        if before != SOURCE_DIGESTS[lang]:
            raise ValueError(f'{lang} staged source changed')
        config = (lane / CONFIG[lang]).read_bytes()
        config_sha = hashlib.sha256(config).hexdigest()
        key = snapshot_key(lane, paths, 'lsp', VERSIONS[lang], config_sha,
                           f'Engram fixture-v1 staged copy; root={lane}; source-set={before}')
        server = Lsp(command_for(root, lang), lane)
        started = time.monotonic()
        try:
            encoding, is_default = init(server, lane)
            open_selected(server, lane, paths, lang)
            if lang == 'rust':
                time.sleep(20)  # cargo workspace loading is asynchronous
            symbols = []
            symbol_positions = {}
            symbol_errors = []
            for path in paths:
                uri = (lane / path).as_uri()
                found = server.request('textDocument/documentSymbol', {'textDocument': {'uri': uri}}, 120)
                for name, kind, symbol_uri, selection, body in flatten_symbols(found, uri):
                    try:
                        token = evidence(lane, paths, symbol_uri, selection, encoding)
                        whole = evidence(lane, paths, symbol_uri, body, encoding)
                        if not (whole['start_byte'] <= token['start_byte'] < token['end_byte'] <= whole['end_byte']):
                            raise ValueError('symbol selection not inside full range')
                        symbol = {'kind': kind, 'name': name, 'token': token, 'body': {
                            'start_byte': whole['start_byte'], 'end_byte': whole['end_byte']}}
                        symbols.append(symbol)
                        symbol_positions[(token['path'], token['start_byte'], token['end_byte'], kind)] = selection['start']
                    except ValueError as exc:
                        symbol_errors.append({'file': path, 'symbol': name, 'reason': str(exc)})
            if full:
                previous = None
                reference_pass_counts = []
                queries_total = 0
                for _ in range(3):
                    full_references, reference_errors, queries = collect_full_references(
                        server, lane, paths, encoding, symbols, symbol_positions)
                    queries_total += queries
                    reference_pass_counts.append(len(full_references))
                    current = (set(full_references), json.dumps(reference_errors, sort_keys=True))
                    if current == previous:
                        break
                    previous = current
                else:
                    raise ValueError(f'non-repeatable LSP references: {reference_pass_counts}; do not publish')
                accepted, conflicts, conflict_examples = exclude_conflicting_sites(full_references)
                verify_snapshot(lane, paths, before, CONFIG[lang], config_sha)
                record = {'schema': 'lsp-code-ir-semantic-prototype-v1', 'language': lang,
                    'provenance': {'source_set_sha256': before, 'snapshot_key': key,
                        'config_sha256': config_sha, 'producer': VERSIONS[lang],
                        'lsp_position_encoding': encoding, 'lsp_encoding_was_default': is_default,
                        'staged_root': str(lane),
                        'selected_paths': paths},
                    'symbols': symbols, 'references': accepted,
                    'reference_pass_counts': reference_pass_counts,
                    'errors': {'document_symbols': symbol_errors, 'references': reference_errors,
                               'conflicting_reference_sites_excluded': conflicts,
                               'conflict_examples': conflict_examples}}
                verify_snapshot(lane, paths, before, CONFIG[lang], config_sha)
                filename, sha256, size = publish_full(root, lang, record)
                print(json.dumps({'language': lang, 'mode': 'full LSP-only rebuild',
                    'selected_files': len(paths), 'document_symbols': len(symbols),
                    'document_symbol_errors': len(symbol_errors), 'reference_queries': queries_total,
                    'reference_seed_symbols': queries, 'reference_pass_counts': reference_pass_counts,
                    'non_reference_impl_symbols': len(symbols) - queries,
                    'reference_candidates': len(full_references), 'references_published': len(accepted),
                    'reference_errors': len(reference_errors), 'conflicting_sites_excluded': conflicts,
                    'conflict_examples': conflict_examples,
                    'snapshot_key': key, 'position_encoding': encoding,
                    'artifact': filename, 'artifact_sha256': sha256, 'artifact_bytes': size,
                    'elapsed_seconds': round(time.monotonic() - started, 2)}), flush=True)
                continue
            facts = []
            discovered = {}
            missing = []
            for row in labels[lang]:
                ref, definition = anchor(lane, row['site']), anchor(lane, row['definition'])
                uri = (lane / ref['path']).as_uri()
                location = server.request('textDocument/definition', {'textDocument': {'uri': uri},
                    'position': {'line': ref['line'], 'character': ref['start']}}, 120)
                if isinstance(location, dict):
                    location = [location]
                if len(location or []) != 1:
                    missing.append({'site': row['site'], 'reason': f'{len(location or [])} definition targets'})
                    continue
                target = location[0]
                target_uri = target.get('targetUri', target.get('uri'))
                target_range = target.get('targetSelectionRange', target.get('range'))
                try:
                    # LSP may not provide originSelectionRange for Location. The
                    # *independently authored* site token is used for this probe.
                    site_range = {'start': {'line': ref['line'], 'character': ref['start']},
                                  'end': {'line': ref['line'], 'character': ref['end']}}
                    site = evidence(lane, paths, uri, site_range, encoding, ref['token'])
                    dest = evidence(lane, paths, target_uri, target_range, encoding, definition['token'])
                    if dest['path'] != definition['path'] or target_range['start']['line'] != definition['line']:
                        raise ValueError('target does not match independently authored path/line')
                    facts.append({'relation': 'references', 'site': site, 'target': dest,
                                  'site_method': 'authored reference token + LSP definition', 'kind': row['kind']})
                    # The authored target is a seed only; these additional
                    # source sites come from live LSP find-references output.
                    refs = server.request('textDocument/references', {'textDocument': {'uri': target_uri},
                        'position': target_range['start'], 'context': {'includeDeclaration': False}}, 120)
                    for item in refs or []:
                        try:
                            source = evidence(lane, paths, item['uri'], item['range'], encoding)
                            if source['slice'] != definition['token']:
                                raise ValueError('reference selection does not equal target identifier')
                            key_ref = (source['path'], source['start_byte'], dest['path'], dest['start_byte'])
                            discovered[key_ref] = {'relation': 'references', 'site': source, 'target': dest,
                                                   'site_method': 'LSP find-references'}
                        except (ValueError, KeyError) as exc:
                            missing.append({'reference_for': row['definition'], 'reason': str(exc)})
                except (ValueError, KeyError) as exc:
                    missing.append({'site': row['site'], 'reason': str(exc)})
            verify_snapshot(lane, paths, before, CONFIG[lang], config_sha)
            # Go manifest has no lines; its proxy checks only path+name.
            # None of these proxies are exact body-span/kind conformance.
            name_line_matches = sum(any(
                symbol['token']['path'] == unit['path'] and
                symbol['token']['slice'] == unit['symbol'].split('.')[-1] and
                (('start_line' not in unit) or
                 unit['start_line'] <= symbol['token']['line'] <= unit['end_line'])
                for symbol in symbols) for unit in manifest['units'])
            print(json.dumps({'language': lang, 'lsp_position_encoding': encoding,
                'lsp_encoding_was_default': is_default, 'snapshot_key': key,
                'source_set_sha256': before, 'config_sha256': config_sha, 'producer': VERSIONS[lang],
                'selected_files': len(paths), 'document_symbols_mapped': len(symbols),
                'document_symbol_errors': symbol_errors[:5], 'document_symbol_error_count': len(symbol_errors),
                'manifest_name_line_proxy': [name_line_matches, len(manifest['units'])],
                'proxy_uses_manifest_line_bounds': lang != 'go',
                'authored_reference_facts': len(facts), 'authored_reference_denominator': len(labels[lang]),
                'discovered_reference_facts': len(discovered), 'missing': missing,
                'sample_fact': facts[0] if facts else None,
                'sample_discovered_fact': next(iter(discovered.values()), None),
                'elapsed_seconds': round(time.monotonic() - started, 2)}), flush=True)
        finally:
            server.close()


def unicode_control(root):
    """Real TS language-server control with astral text before a symbol + CRLF."""
    with tempfile.TemporaryDirectory(prefix='lsp-unicode-', dir=root) as temporary:
        _unicode_control(root, pathlib.Path(temporary))


def inspect_generated(root, lang):
    artifact = json.loads((root / f'lsp-full-{lang}.json').read_text())
    for example in artifact['errors']['conflict_examples']:
        details = []
        for path, start, end in example['targets']:
            details.extend({'name': symbol['name'], 'kind': symbol['kind'], 'slice': symbol['token']['slice'],
                            'path': path, 'start_byte': start} for symbol in artifact['symbols']
                           if symbol['token']['path'] == path and symbol['token']['start_byte'] == start
                           and symbol['token']['end_byte'] == end)
        print(json.dumps({'site': example['site'], 'target_symbols': details}))


def check_labels(root, languages):
    """Evaluate full artifacts AFTER extraction; labels are never its inputs."""
    labels = json.loads(LABELS.read_text())
    for lang in languages:
        lane = root / lang
        record = json.loads((root / f'lsp-full-{lang}.json').read_text())
        paths = selected(lang)[2]
        provenance = record['provenance']
        verify_snapshot(lane, paths, provenance['source_set_sha256'], CONFIG[lang], provenance['config_sha256'])
        correct = 0
        missed = []
        for row in labels[lang]:
            site, target = anchor(lane, row['site']), anchor(lane, row['definition'])
            def start_byte(item):
                lines = (lane / item['path']).read_bytes().splitlines(keepends=True)
                return sum(map(len, lines[:item['line']])) + item['start']
            matching_site = [fact for fact in record['references'] if fact['site']['path'] == site['path']
                             and fact['site']['start_byte'] == start_byte(site)]
            matched = any(fact['target']['path'] == target['path'] and
                          fact['target']['start_byte'] == start_byte(target) for fact in matching_site)
            correct += matched
            if not matched:
                missed.append({'site': row['site'], 'expected': row['definition'],
                               'actual': [{'path': fact['target']['path'], 'byte': fact['target']['start_byte']}
                                          for fact in matching_site]})
        print(json.dumps({'language': lang, 'full_artifact_authored_reference_matches': correct,
                          'denominator': len(labels[lang]), 'misses': missed}), flush=True)


def compare_generated(first_root, second_root, lang):
    def links(root):
        record = json.loads((root / f'lsp-full-{lang}.json').read_text())
        return {(fact['site']['path'], fact['site']['start_byte'], fact['target']['path'],
                 fact['target']['start_byte']) for fact in record['references']}
    a, b = links(first_root), links(second_root)
    print(json.dumps({'language': lang, 'only_first': sorted(a - b)[:20],
                      'only_second': sorted(b - a)[:20], 'first_count': len(a), 'second_count': len(b)}))


def _unicode_control(root, control):
    target = 'const marker = "é🚀"; export function target(): number { return 1; }\r\n'
    consumer = 'import { target } from "./target.js";\r\nexport const result = "é🚀" + String(target());\r\n'
    (control / 'target.ts').write_bytes(target.encode())
    (control / 'consumer.ts').write_bytes(consumer.encode())
    (control / 'tsconfig.json').write_text(json.dumps({'compilerOptions': {'target': 'ES2022', 'module': 'commonjs'}, 'include': ['*.ts']}))
    files = ['target.ts', 'consumer.ts']
    server = Lsp(command_for(root, 'typescript'), control)
    try:
        encoding, _ = init(server, control)
        open_selected(server, control, files, 'typescript')
        uri = (control / 'target.ts').as_uri()
        symbols = list(flatten_symbols(server.request('textDocument/documentSymbol',
            {'textDocument': {'uri': uri}}, 120), uri))
        match = [(symbol_uri, selection) for name, _, symbol_uri, selection, _ in symbols if name == 'target']
        if len(match) != 1:
            raise ValueError(f'expected one target declaration: {symbols}')
        declaration = evidence(control, files, *match[0], encoding, expected='target')
        expected_declaration = target.encode().index(b'target():')
        if declaration['start_byte'] != expected_declaration:
            raise ValueError('unicode-adjusted declaration byte position incorrect')
        references = server.request('textDocument/references', {'textDocument': {'uri': uri},
            'position': match[0][1]['start'], 'context': {'includeDeclaration': False}}, 120)
        sites = [evidence(control, files, loc['uri'], loc['range'], encoding, expected='target')
                 for loc in references or []]
        call = [site for site in sites if site['path'] == 'consumer.ts' and site['line'] == 2]
        expected_call = consumer.encode().rindex(b'target()')
        if len(call) != 1 or call[0]['start_byte'] != expected_call:
            raise ValueError('unicode-adjusted CRLF call byte position incorrect')
        print(json.dumps({'control': 'typescript LSP real Unicode + CRLF', 'negotiated': encoding,
            'source_set_sha256': digest(control, files), 'declaration': declaration,
            'call': call[0], 'reference_count': len(sites),
            'declaration_lsp_character': match[0][1]['start']['character'],
            'call_expected_utf8_byte': expected_call}), flush=True)
    finally:
        server.close()


if __name__ == '__main__':
    root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    if '--unicode-control' in sys.argv[2:]:
        unicode_control(root)
    elif '--inspect' in sys.argv[2:]:
        inspect_generated(root, sys.argv[3])
    elif '--check-labels' in sys.argv[2:]:
        check_labels(root, tuple(arg for arg in sys.argv[2:] if arg != '--check-labels') or LANGS)
    elif '--compare' in sys.argv[2:]:
        compare_generated(root, pathlib.Path(sys.argv[3]).resolve(strict=True), sys.argv[4])
    else:
        args = sys.argv[2:]
        run(root, tuple(arg for arg in args if arg != '--full') or LANGS, full='--full' in args)
