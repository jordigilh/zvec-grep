"""Actual Go/TS/Python SCIP producer Unicode+CRLF position-encoding controls.

python3 scripts/scip_nonrust_position_controls.py ROOT
ROOT contains locally installed producers, pinned Node 24, and generated
scip_pb2.py. All source/output is created in a disposable child of ROOT.
This is diagnostic only: it never rewrites a producer's SCIP metadata.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.parse

if __package__:
    from scripts.scip_spike import digest, range_bytes
else:
    from scip_spike import digest, range_bytes


def expected_byte_offset(raw, token, last=False):
    return raw.rindex(token) if last else raw.index(token)


def candidate(index, lane, paths, encoding, definition, reference):
    docs = {d.relative_path: d for d in index.documents}
    counts = {'occurrences': 0, 'mapped': 0, 'invalid': 0, 'definition_exact': 0,
              'reference_exact': 0, 'same_symbol': False}
    hits = {'definition': [], 'reference': []}
    for name, doc in docs.items():
        raw = (lane / name).read_bytes()
        for occ in doc.occurrences:
            counts['occurrences'] += 1
            try:
                a, b = range_bytes(raw.decode('utf-8'), occ, encoding)
            except ValueError:
                counts['invalid'] += 1
                continue
            counts['mapped'] += 1
            if name == definition[0] and a == definition[1] and b == definition[1] + 6 and occ.symbol_roles & 1:
                hits['definition'].append((occ.symbol, list(occ.range)))
            if name == reference[0] and a == reference[1] and b == reference[1] + 6 and occ.symbol and not occ.symbol_roles & 1:
                hits['reference'].append((occ.symbol, list(occ.range)))
    counts['definition_exact'] = len(hits['definition'])
    counts['reference_exact'] = len(hits['reference'])
    counts['same_symbol'] = any(a[0] == b[0] for a in hits['definition'] for b in hits['reference'])
    counts['definition_ranges'] = [entry[1] for entry in hits['definition'][:2]]
    counts['reference_ranges'] = [entry[1] for entry in hits['reference'][:2]]
    return counts


def run(root, languages=('typescript', 'python', 'go'), go_binary=None):
    sys.path.insert(0, str(root))
    import scip_pb2
    node = root / 'node24/node_modules/node/bin/node'
    ts = root / 'node_modules/@sourcegraph/scip-typescript/dist/src/main.js'
    py = root / 'node_modules/@sourcegraph/scip-python/index.js'
    go = pathlib.Path(go_binary) if go_binary else root / 'scip-go'
    required = [root / 'scip_pb2.py']
    required.extend({'typescript': [node, ts], 'python': [node, py], 'go': [go]}[lang]
                    for lang in languages)
    for executable in [item for group in required for item in (group if isinstance(group, list) else [group])]:
        if not executable.exists():
            raise FileNotFoundError(f'missing producer/tool: {executable}')
    with tempfile.TemporaryDirectory(prefix='scip-position-', dir=root) as temp:
        stage = pathlib.Path(temp)
        config = {
            'typescript': {
                'target.ts': 'export const marker = "é🚀"; export function target(): number { return 1; }\r\n',
                'consumer.ts': 'import { target } from "./target.js";\r\nexport const value = "é🚀" + String(target());\r\n',
                'tsconfig.json': json.dumps({'compilerOptions': {'target': 'ES2022', 'module': 'commonjs'}, 'include': ['*.ts']}),
            },
            'python': {
                'target.py': 'marker = "é🚀"; target = 1\r\n',
                'consumer.py': 'from target import target\r\nmarker = "é🚀"; result = target\r\n',
                'pyrightconfig.json': json.dumps({'include': ['*.py'], 'pythonVersion': '3.12',
                                                   'pythonPath': '/opt/homebrew/opt/python@3.12/libexec/bin/python3'}),
            },
            'go': {
                'target.go': 'package control\r\n/* é🚀 */ func target() {}\r\n',
                'consumer.go': 'package control\r\n/* é🚀 */ func caller() { target() }\r\n',
                'go.mod': 'module example.com/scip-unicode-control\n\ngo 1.26\n',
            },
        }
        for lang, files in config.items():
            if lang not in languages:
                continue
            lane = stage / lang
            lane.mkdir()
            for name, text in files.items():
                (lane / name).write_bytes(text.encode('utf-8'))
            project = lane.resolve(strict=True)
            output = lane / 'index.scip'
            commands = {
                'typescript': [str(node), str(ts), 'index', '--cwd', str(project), '--output', str(output), '--no-progress-bar'],
                'python': [str(node), str(py), 'index', '--cwd', str(project), '--project-name', 'scip-unicode-control',
                           '--project-version', 'fixture-v1', '--output', str(output)],
                'go': [str(go), 'index', './...', '--module-root', '.', '--repository-remote',
                       'synthetic/scip-unicode-control', '--module-version', 'fixture-v1', '--skip-tests',
                       '--output', str(output)],
            }
            env = os.environ.copy()
            if lang == 'python':
                env['PATH'] = '/opt/homebrew/opt/python@3.12/libexec/bin:' + env.get('PATH', '')
            started = time.monotonic()
            process = subprocess.run(commands[lang], cwd=project, env=env, capture_output=True, text=True, timeout=90)
            if process.returncode != 0 or not output.exists():
                print(json.dumps({'language': lang, 'error': 'producer failed', 'exit_code': process.returncode,
                                  'stderr': process.stderr[-800:], 'stdout': process.stdout[-800:]}), flush=True)
                continue
            data = output.read_bytes()
            index = scip_pb2.Index()
            index.ParseFromString(data)
            paths = sorted(name for name in files if name.endswith({'typescript': '.ts', 'python': '.py', 'go': '.go'}[lang]))
            docs = {d.relative_path: d for d in index.documents}
            if sorted(docs) != paths:
                raise ValueError(f'{lang} indexed wrong file set: {sorted(docs)}')
            root_url = urllib.parse.urlparse(index.metadata.project_root)
            if root_url.scheme != 'file' or pathlib.Path(urllib.parse.unquote(root_url.path)).resolve(strict=True) != project:
                raise ValueError('SCIP root mismatch')
            decl_name = 'target' + {'typescript': '.ts', 'python': '.py', 'go': '.go'}[lang]
            ref_name = 'consumer' + {'typescript': '.ts', 'python': '.py', 'go': '.go'}[lang]
            declaration_raw = (lane / decl_name).read_bytes()
            reference_raw = (lane / ref_name).read_bytes()
            declaration = [decl_name, expected_byte_offset(declaration_raw, b'target' + (b'()' if lang != 'python' else b' ='))]
            reference = [ref_name, expected_byte_offset(reference_raw, b'target' + (b'()' if lang in ('typescript', 'go') else b''), last=True)]
            results = {str(enc): candidate(index, lane, paths, enc, declaration, reference) for enc in (1, 2, 3)}
            print(json.dumps({'language': lang, 'tool': {'name': index.metadata.tool_info.name,
                'version': index.metadata.tool_info.version}, 'node_version': '24.21.0' if lang in ('typescript', 'python') else None,
                'python_path': '/opt/homebrew/opt/python@3.12/libexec/bin/python3' if lang == 'python' else None,
                'declared_position_encodings': {name: doc.position_encoding for name, doc in docs.items()},
                'source_set_sha256': digest(lane, paths), 'config_sha256': hashlib.sha256(
                    (lane / {'typescript': 'tsconfig.json', 'python': 'pyrightconfig.json', 'go': 'go.mod'}[lang]).read_bytes()).hexdigest(),
                'scip_sha256': hashlib.sha256(data).hexdigest(), 'index_bytes': len(data),
                'typed_primary_occurrences': sum(bool(o.WhichOneof('typed_range')) for d in docs.values() for o in d.occurrences),
                'definition_expected_bytes': declaration, 'reference_expected_bytes': reference,
                'candidate_encodings': results, 'producer_warnings': process.stderr[-400:],
                'elapsed_seconds': round(time.monotonic() - started, 2)}), flush=True)


if __name__ == '__main__':
    args = sys.argv[2:]
    go_binary = None
    if '--go-binary' in args:
        i = args.index('--go-binary')
        go_binary = args[i + 1]
        del args[i:i + 2]
    run(pathlib.Path(sys.argv[1]).resolve(strict=True), tuple(args) or ('typescript', 'python', 'go'), go_binary)
