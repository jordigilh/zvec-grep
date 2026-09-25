"""Reproduce the isolated SCIP fixture staging and inspect actual protobuf indexes.

stage ROOT copies only manifest-selected source into ROOT/{go,python,rust,typescript}.
decode ROOT reads indexes from each lane and emits compact JSON to stdout.
"""
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys
import urllib.request

FIXTURES = pathlib.Path('/Users/jgil/go/src/github.com/jordigilh/engram/benchmarks/semantic_search/fixtures')
LANGS = ('typescript', 'python', 'rust', 'go')
EXT = {'typescript': '.ts', 'python': '.py', 'rust': '.rs', 'go': '.go'}
PROTO_URL = 'https://raw.githubusercontent.com/sourcegraph/scip/main/scip.proto'


def selected(lang):
    source = FIXTURES / f'{lang}-workflow-discovery-v1'
    manifest = json.loads((source / 'manifest.json').read_text())
    paths = sorted({u['path'] for u in manifest['units']})
    return source, manifest, paths


def digest(root, paths):
    """Length-framed complete file-set identity, including path and content."""
    h = hashlib.sha256()
    for name in sorted(paths):
        p = pathlib.PurePosixPath(name)
        if p.is_absolute() or '..' in p.parts or str(p) != name:
            raise ValueError(f'unsafe path {name}')
        data = (root / name).read_bytes()
        for field in (name.encode(), hashlib.sha256(data).digest()):
            h.update(len(field).to_bytes(8, 'big'))
            h.update(field)
    return h.hexdigest()


def attest(root, paths, expected):
    if not paths or digest(root, paths) != expected:
        raise ValueError('unverified or stale full-file-set snapshot')
    return True


def snapshot_key(root, paths, producer, version, config_sha256, git_state):
    """Bind selection/content to producer configuration and worktree provenance."""
    if not all((paths, producer, version, config_sha256, git_state)):
        raise ValueError('incomplete snapshot attestation')
    identity = {'files': digest(root, paths), 'paths': sorted(paths), 'producer': producer,
                'version': version, 'config_sha256': config_sha256, 'git_state': git_state}
    return hashlib.sha256(json.dumps(identity, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def join_snapshot(index_key, active_key):
    if not index_key or not active_key or index_key != active_key:
        raise ValueError('unverified cross-file snapshot')
    return True


def stage(root):
    root.mkdir(parents=True, exist_ok=False)
    for lang in LANGS:
        source, manifest, paths = selected(lang)
        dst = root / lang
        dst.mkdir()
        for name in paths:
            target = dst / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, target)
        for config in ('Cargo.toml', 'go.mod'):
            if (source / config).exists():
                shutil.copyfile(source / config, dst / config)
        if lang == 'typescript':
            (dst / 'tsconfig.json').write_text(json.dumps({'compilerOptions': {'target': 'ES2022', 'module': 'commonjs', 'strict': True}, 'include': ['**/*.ts']}))
        if lang == 'python':
            (dst / 'pyrightconfig.json').write_text(json.dumps({'include': ['.'], 'exclude': ['**/test_*.py'], 'typeCheckingMode': 'basic'}))
        assert digest(dst, paths) == digest(source, paths)
        print(json.dumps({'language': lang, 'source_files': len(paths), 'units': len(manifest['units']), 'source_set_sha256': digest(dst, paths)}))


def position(text, line, col, encoding):
    """Return UTF-8 byte offset; reject split code units and line terminators."""
    if encoding not in (1, 2, 3):
        raise ValueError('unknown position encoding')
    lines = text.splitlines(keepends=True)
    if text.endswith(('\n', '\r')):
        lines.append('')
    if line < 0 or line >= len(lines) or col < 0:
        raise ValueError('invalid line/column')
    prefix = sum(len(s.encode('utf-8')) for s in lines[:line])
    raw = lines[line]
    content = raw.removesuffix('\n').removesuffix('\r')
    codec = {1: 'utf-8', 2: 'utf-16-le', 3: 'utf-32-le'}[encoding]
    unit = {1: 1, 2: 2, 3: 4}[encoding]
    b = content.encode(codec)
    if col * unit > len(b):
        raise ValueError('column beyond line')
    try:
        decoded = b[:col * unit].decode(codec)
    except UnicodeDecodeError as exc:
        raise ValueError('split code point') from exc
    return prefix + len(decoded.encode('utf-8'))


def range_bytes(text, occurrence, encoding, enclosing=False):
    if enclosing:
        variant = occurrence.WhichOneof('typed_enclosing_range')
        legacy = occurrence.enclosing_range
    else:
        variant = occurrence.WhichOneof('typed_range')
        legacy = occurrence.range
    if variant:
        r = getattr(occurrence, variant)
        coords = (r.start_line, r.start_character, r.end_line, r.end_character) if 'multi_line' in variant else (r.line, r.start_character, r.line, r.end_character)
    elif len(legacy) == 3:
        coords = (legacy[0], legacy[1], legacy[0], legacy[2])
    elif len(legacy) == 4:
        coords = tuple(legacy)
    else:
        raise ValueError('missing/invalid range')
    a, b = position(text, coords[0], coords[1], encoding), position(text, coords[2], coords[3], encoding)
    if b < a:
        raise ValueError('reversed range')
    return a, b


def decode(root):
    sys.path.insert(0, str(root))
    import scip_pb2
    for lang in LANGS:
        src, manifest, paths = selected(lang)
        lane = root / lang
        index = lane / 'index.scip'
        if not index.exists():
            print(json.dumps({'language': lang, 'error': 'index.scip missing'}))
            continue
        msg = scip_pb2.Index()
        data = index.read_bytes()
        msg.ParseFromString(data)
        docs = {d.relative_path: d for d in msg.documents}
        counts = {'occurrences': 0, 'raw_definitions': 0, 'raw_symbol_references': 0, 'definitions': 0, 'references': 0, 'empty': 0, 'invalid': 0, 'invalid_reasons': {}, 'enclosing': 0, 'relationships': 0, 'implementations': 0, 'external_symbols': len(msg.external_symbols), 'local_reference_sites': 0, 'unresolved_reference_sites': 0, 'manifest_name_line_matches': 0}
        examples = []
        defs = {}
        matched_units = set()
        for doc in msg.documents:
            path = lane / doc.relative_path
            if doc.relative_path not in paths or not path.is_file() or path.is_symlink():
                counts['invalid'] += len(doc.occurrences)
                continue
            text = path.read_text()
            raw = path.read_bytes()
            for sym in doc.symbols:
                counts['relationships'] += len(sym.relationships)
                counts['implementations'] += sum(r.is_implementation for r in sym.relationships)
            for occ in doc.occurrences:
                counts['occurrences'] += 1
                if occ.symbol:
                    counts['raw_definitions' if occ.symbol_roles & 1 else 'raw_symbol_references'] += 1
                try:
                    a, b = range_bytes(text, occ, doc.position_encoding)
                    if a == b:
                        counts['empty'] += 1
                except ValueError as exc:
                    counts['invalid'] += 1
                    key = str(exc)
                    counts['invalid_reasons'][key] = counts['invalid_reasons'].get(key, 0) + 1
                    continue
                if occ.WhichOneof('typed_enclosing_range') or occ.enclosing_range:
                    try:
                        x, y = range_bytes(text, occ, doc.position_encoding, True)
                        if x > a or y < b:
                            raise ValueError('enclosing range does not contain occurrence')
                        counts['enclosing'] += 1
                    except ValueError as exc:
                        key = 'enclosing: ' + str(exc)
                        counts['invalid_reasons'][key] = counts['invalid_reasons'].get(key, 0) + 1
                if occ.symbol:
                    if occ.symbol_roles & 1:
                        counts['definitions'] += 1
                        defs.setdefault((doc.relative_path if occ.symbol.startswith('local ') else '', occ.symbol), []).append((doc.relative_path, a, b))
                        token = raw[a:b].decode('utf-8')
                        for u in manifest['units']:
                            if u['path'] == doc.relative_path and u['start_line'] <= raw[:a].count(b'\n') + 1 <= u['end_line'] and token == u['symbol'].split('.')[-1]:
                                counts['manifest_name_line_matches'] += 1
                                matched_units.add(u['unit_id'])
                                break
                    else:
                        counts['references'] += 1
                    if len(examples) < 4 and 0 < b - a < 60:
                        examples.append({'path': doc.relative_path, 'slice': raw[a:b].decode('utf-8'), 'bytes': [a, b], 'symbol': occ.symbol[:120], 'role': occ.symbol_roles})
        for doc in msg.documents:
            if doc.relative_path not in paths:
                continue
            for occ in doc.occurrences:
                if occ.symbol and not (occ.symbol_roles & 1):
                    key = (doc.relative_path if occ.symbol.startswith('local ') else '', occ.symbol)
                    counts['local_reference_sites' if key in defs else 'unresolved_reference_sites'] += 1
        print(json.dumps({'language': lang, 'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data), 'tool': {'name': msg.metadata.tool_info.name, 'version': msg.metadata.tool_info.version, 'arguments': list(msg.metadata.tool_info.arguments)}, 'project_root': msg.metadata.project_root, 'documents': len(docs), 'selected_files': len(paths), 'missing': sorted(set(paths) - set(docs)), 'extra': sorted(set(docs) - set(paths)), 'encodings': {str(k): sum(d.position_encoding == k for d in msg.documents) for k in (0, 1, 2, 3)}, 'range_forms': {'typed': sum(bool(o.WhichOneof('typed_range')) for d in msg.documents for o in d.occurrences), 'legacy': sum(bool(o.range) for d in msg.documents for o in d.occurrences)}, 'unique_manifest_name_line_units': len(matched_units), 'counts': counts, 'examples': examples}))


def schema(root):
    data = urllib.request.urlopen(PROTO_URL, timeout=30).read()
    (root / 'scip.proto').write_bytes(data)
    subprocess.run(['protoc', f'--proto_path={root}', f'--python_out={root}', 'scip.proto'], cwd=root, check=True)
    print('schema_sha256', hashlib.sha256(data).hexdigest())


def probe(root):
    """Cross-file dirty target changed while the referring source stays identical."""
    source = root / 'go'
    target = root / 'probe-go'
    shutil.copytree(source, target, ignore=shutil.ignore_patterns('index.scip'))
    paths = selected('go')[2]
    before = digest(target, paths)
    referring = target / 'internal/tools/list_workflows.go'
    reference_hash = hashlib.sha256(referring.read_bytes()).hexdigest()
    changed = target / 'internal/discovery/state.go'
    contents = changed.read_text()
    assert 'return ok\n' in contents
    changed.write_text(contents.replace('return ok\n', 'return ok && workflowID != "dirty-probe"\n', 1))
    after = digest(target, paths)
    assert before != after and hashlib.sha256(referring.read_bytes()).hexdigest() == reference_hash
    print(json.dumps({'probe': 'go cross-file dirty edit', 'baseline_source_set': before, 'dirty_source_set': after, 'referring_file_unchanged_sha256': reference_hash, 'stale_rejected': before != after}))
    subprocess.run([str(root / 'scip-go'), 'index', './...', '--module-root', '.', '--repository-remote', 'synthetic/workflow', '--module-version', 'fixture-v1', '--skip-tests', '--output', 'index.scip'], cwd=target, check=True)
    data = (target / 'index.scip').read_bytes()
    print(json.dumps({'probe': 'fresh dirty index', 'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data), 'attested': bool(after)}))


def samples(root):
    sys.path.insert(0, str(root))
    import scip_pb2
    for lang in LANGS:
        msg = scip_pb2.Index()
        msg.ParseFromString((root / lang / 'index.scip').read_bytes())
        definitions = {o.symbol: (d.relative_path, list(o.range)) for d in msg.documents for o in d.occurrences if o.symbol and not o.symbol.startswith('local ') and o.symbol_roles & 1}
        candidates = []
        for d in msg.documents:
            for o in d.occurrences:
                if o.symbol and not (o.symbol_roles & 1) and not o.symbol.startswith('local ') and o.symbol in definitions and definitions[o.symbol][0] != d.relative_path:
                    candidates.append({'reference_path': d.relative_path, 'range': list(o.range), 'definition': definitions[o.symbol], 'symbol': o.symbol})
        relations = [{'path': d.relative_path, 'symbol': s.symbol, 'target': r.symbol, 'implementation': r.is_implementation} for d in msg.documents for s in d.symbols for r in s.relationships]
        bad_enclosing = []
        if lang == 'rust':
            for d in msg.documents:
                raw = (root / lang / d.relative_path).read_bytes()
                text = raw.decode()
                for o in d.occurrences:
                    if not o.enclosing_range:
                        continue
                    a, b = range_bytes(text, o, d.position_encoding)
                    x, y = range_bytes(text, o, d.position_encoding, True)
                    if x > a or y < b:
                        bad_enclosing.append({'path': d.relative_path, 'token': raw[a:b].decode(), 'token_range': list(o.range), 'enclosing_range': list(o.enclosing_range)})
        print(json.dumps({'language': lang, 'cross_file_samples': candidates[:3], 'cross_file_sites': len(candidates), 'relationships': relations[:5], 'bad_enclosing_samples': bad_enclosing[:3]}))


if __name__ == '__main__':
    action, root_name = sys.argv[1:3]
    root = pathlib.Path(root_name).resolve()
    if action == 'stage':
        stage(root)
    elif action == 'decode':
        decode(root)
    elif action == 'schema':
        schema(root)
    elif action == 'probe':
        probe(root)
    elif action == 'samples':
        samples(root)
    else:
        raise ValueError(action)
