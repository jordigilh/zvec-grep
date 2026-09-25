"""Compare independently authored source-site labels with SCIP and local LSP servers.

Usage: python3 scripts/scip_reference_probe.py ROOT
Writes one compact JSON object/line to stdout; never changes the fixture roots.
"""
import hashlib
import json
import os
import pathlib
import selectors
import subprocess
import sys
import time
import urllib.parse

if __package__:
    from scripts.scip_spike import LANGS, digest, selected
else:
    from scip_spike import LANGS, digest, selected

LABELS = pathlib.Path(__file__).resolve().parent.parent / 'docs/scip-reference-sites.json'
SOURCE_DIGESTS = {
    'typescript': '6c66c5498b7764e1291739ba8bcf21ab853e17691793d7c7d24e18dd7a6c9482',
    'python': '6db37eaae3a00ad08b68843280720e0cd7eee83e1836f2d34a9195f4bd0476ca',
    'rust': 'a9f5020761eb5c66291715068224f152c120545069c090f5746cc086d55fefc3',
    'go': '13ca96abdeb8ef919a746f1ec1afb3ae5b6d59e8b0282d98109bc2b6a8c4f53d',
}


def anchor(root, label):
    path, line, token, nth = label
    content = (root / path).read_text().splitlines()
    assert 1 <= line <= len(content), label
    text = content[line - 1]
    assert text.isascii(), (label, 'non-ASCII control needs declared encoding')
    start = -1
    for _ in range(nth + 1):
        start = text.find(token, start + 1)
        assert start >= 0, (label, text)
    assert (start == 0 or not (text[start - 1].isalnum() or text[start - 1] == '_')), label
    assert (start + len(token) == len(text) or not (text[start + len(token)].isalnum() or text[start + len(token)] == '_')), label
    return {'path': path, 'line': line - 1, 'start': start, 'end': start + len(token), 'token': token}


def occ_coords(occ):
    if occ.WhichOneof('typed_range'):
        obj = getattr(occ, occ.WhichOneof('typed_range'))
        if 'multi' in occ.WhichOneof('typed_range'):
            return obj.start_line, obj.start_character, obj.end_line, obj.end_character
        return obj.line, obj.start_character, obj.line, obj.end_character
    r = list(occ.range)
    return (r[0], r[1], r[0], r[2]) if len(r) == 3 else tuple(r)


def scip_link(index, site, target):
    """Only compare ASCII fixture coordinates; unspecified encoding stays unaccepted."""
    documents = {d.relative_path: d for d in index.documents}
    document = documents.get(site['path'])
    if not document:
        return {'status': 'missing_document'}
    matches = [o for o in document.occurrences if o.symbol and not (o.symbol_roles & 1) and occ_coords(o) == (site['line'], site['start'], site['line'], site['end'])]
    if len(matches) != 1:
        return {'status': 'missing_or_ambiguous_site', 'occurrences': len(matches)}
    symbol = matches[0].symbol
    destinations = []
    for doc in index.documents:
        if symbol.startswith('local ') and doc.relative_path != site['path']:
            continue
        for o in doc.occurrences:
            if o.symbol == symbol and o.symbol_roles & 1:
                start_line, start, end_line, end = occ_coords(o)
                destinations.append({'path': doc.relative_path, 'line': start_line, 'start': start, 'end_line': end_line, 'end': end})
    good = [d for d in destinations if d['path'] == target['path'] and d['line'] == target['line'] and (d['start'] <= target['start'] < d['end'] or d['end_line'] > d['line'] and d['start'] <= target['start'])]
    return {'status': 'match' if good else 'wrong_target' if destinations else 'no_local_target',
            'symbol': symbol, 'destinations': destinations[:4], 'declared_encoding': document.position_encoding}


class Lsp:
    def __init__(self, command, root):
        self.process = subprocess.Popen(command, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, bufsize=0)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.pending = bytearray()
        self.counter = 0

    def send(self, data):
        raw = json.dumps(data, separators=(',', ':')).encode()
        self.process.stdin.write(b'Content-Length: ' + str(len(raw)).encode() + b'\r\n\r\n' + raw)
        self.process.stdin.flush()

    def request(self, method, params, timeout=100):
        self.counter += 1
        request_id = self.counter
        self.send({'jsonrpc': '2.0', 'id': request_id, 'method': method, 'params': params})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            message = self.receive(deadline)
            if 'method' in message and 'id' in message:
                answer = [None] * len(message.get('params', {}).get('items', [])) if message['method'] == 'workspace/configuration' else None
                self.send({'jsonrpc': '2.0', 'id': message['id'], 'result': answer})
            elif message.get('id') == request_id:
                if 'error' in message:
                    raise RuntimeError(f'{method}: {message["error"]}')
                return message.get('result')
        raise TimeoutError(method)

    def receive(self, deadline):
        while time.monotonic() < deadline:
            split = self.pending.find(b'\r\n\r\n')
            if split >= 0:
                headers = self.pending[:split].decode('ascii').split('\r\n')
                length = next(int(h.split(':', 1)[1]) for h in headers if h.lower().startswith('content-length:'))
                if len(self.pending) >= split + 4 + length:
                    body = bytes(self.pending[split + 4:split + 4 + length])
                    del self.pending[:split + 4 + length]
                    return json.loads(body)
            events = self.selector.select(max(0, deadline - time.monotonic()))
            if not events:
                break
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError(f'LSP exited {self.process.poll()}')
            self.pending.extend(chunk)
        raise TimeoutError('LSP response')

    def close(self):
        try:
            self.request('shutdown', None, 5)
            self.send({'jsonrpc': '2.0', 'method': 'exit'})
            self.process.wait(timeout=5)
        except (TimeoutError, RuntimeError, subprocess.TimeoutExpired):
            self.process.terminate()
            self.process.wait(timeout=5)
        self.selector.close()


def location_matches(locations, expected, root):
    if isinstance(locations, dict):
        locations = [locations]
    found = []
    for location in locations or []:
        uri = location.get('targetUri', location.get('uri', ''))
        uri_path = pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(uri).path)).resolve()
        area = location.get('targetSelectionRange', location.get('range', {})).get('start', {})
        found.append({'path': str(uri_path), 'line': area.get('line'), 'character': area.get('character')})
    want = (root / expected['path']).resolve()
    return any(d['path'] == str(want) and d['line'] == expected['line'] and expected['start'] <= d['character'] < expected['end'] for d in found), found[:5]


def audit_cross_file(server, index, lane):
    """Enumerate every non-local cross-file reference candidate, not just authored labels.

    For unspecified SCIP encoding, ASCII-only coordinate interpretation remains
    a diagnostic comparison and MUST NOT be imported as a source-mapped fact.
    """
    definitions = {}
    for doc in index.documents:
        for occ in doc.occurrences:
            if occ.symbol and not occ.symbol.startswith('local ') and occ.symbol_roles & 1:
                definitions.setdefault(occ.symbol, []).append((doc.relative_path, occ_coords(occ)))
    counts = {'candidate_sites': 0, 'tested': 0, 'agree': 0, 'disagree': 0,
              'no_lsp_definition': 0, 'ambiguous_scip_target': 0, 'coarse_or_empty_definition': 0,
              'invalid_site': 0}
    discrepancies = []
    excluded_examples = []
    for doc in index.documents:
        raw = (lane / doc.relative_path).read_bytes()
        lines = raw.splitlines()
        for occ in doc.occurrences:
            if not occ.symbol or occ.symbol_roles & 1 or occ.symbol.startswith('local '):
                continue
            dests = [(p, coords) for p, coords in definitions.get(occ.symbol, []) if p != doc.relative_path]
            if not dests:
                continue
            counts['candidate_sites'] += 1
            if len(dests) != 1:
                counts['ambiguous_scip_target'] += 1
                if len(excluded_examples) < 4:
                    excluded_examples.append({'reason': 'ambiguous_scip_target', 'site': doc.relative_path, 'range': list(occ.range), 'symbol': occ.symbol, 'targets': dests[:3]})
                continue
            path, dest = dests[0]
            sl, sc, el, ec = occ_coords(occ)
            if not (0 <= sl < len(lines) and sl == el and 0 <= sc < ec <= len(lines[sl]) and lines[sl].isascii()):
                counts['invalid_site'] += 1
                continue
            # Multi-line/module-wide and zero-length definitions cannot verify
            # a precise identifier target, even if navigation reaches the file.
            if dest[0] != dest[2] or dest[1] == dest[3]:
                counts['coarse_or_empty_definition'] += 1
                if len(excluded_examples) < 4:
                    excluded_examples.append({'reason': 'coarse_or_empty_definition', 'site': doc.relative_path, 'symbol': occ.symbol, 'target': [path, dest]})
                continue
            site = {'path': doc.relative_path, 'line': sl, 'start': sc, 'end': ec}
            target = {'path': path, 'line': dest[0], 'start': dest[1], 'end': dest[3]}
            uri = (lane / site['path']).as_uri()
            found = server.request('textDocument/definition', {'textDocument': {'uri': uri},
                'position': {'line': sl, 'character': sc}}, 120)
            agrees, locations = location_matches(found, target, lane)
            counts['tested'] += 1
            if agrees:
                counts['agree'] += 1
            else:
                key = 'disagree' if found else 'no_lsp_definition'
                counts[key] += 1
                discrepancies.append({'site': site, 'token': lines[sl][sc:ec].decode(), 'scip_target': target,
                                      'lsp_targets': locations, 'symbol': occ.symbol[:130]})
    return {'counts': counts, 'discrepancies': discrepancies, 'excluded_examples': excluded_examples}


def run(root, languages=LANGS, audit=False):
    sys.path.insert(0, str(root))
    import scip_pb2
    labels_data = LABELS.read_bytes()
    labels = json.loads(labels_data)
    tools = root / 'lsp-tools/node_modules'
    commands = {
        'typescript': ['node', str(tools / 'typescript-language-server/lib/cli.mjs'), '--stdio'],
        'python': ['node', str(tools / 'pyright/langserver.index.js'), '--stdio'],
        'rust': ['rust-analyzer'],
        'go': ['gopls', 'serve'],
    }
    print(json.dumps({'label_sha256': hashlib.sha256(labels_data).hexdigest(), 'labels_per_language': {lang: len(labels[lang]) for lang in LANGS}}), flush=True)
    for lang in languages:
        lane = (root / lang).resolve()
        assert digest(lane, selected(lang)[2]) == SOURCE_DIGESTS[lang], f'{lang} staged source changed'
        msg = scip_pb2.Index()
        index_bytes = (lane / 'index.scip').read_bytes()
        msg.ParseFromString(index_bytes)
        server = Lsp(commands[lang], lane)
        try:
            root_uri = lane.as_uri()
            response = server.request('initialize', {'processId': os.getpid(), 'rootPath': str(lane), 'rootUri': root_uri,
                         'workspaceFolders': [{'uri': root_uri, 'name': lang}],
                         'capabilities': {'general': {'positionEncodings': ['utf-16']}, 'textDocument': {'definition': {'linkSupport': True}, 'references': {}}, 'workspace': {'workspaceFolders': True}}}, 120)
            server.send({'jsonrpc': '2.0', 'method': 'initialized', 'params': {}})
            opened = set(selected(lang)[2])
            for path in sorted(opened):
                server.send({'jsonrpc': '2.0', 'method': 'textDocument/didOpen', 'params': {'textDocument': {
                    'uri': (lane / path).as_uri(), 'languageId': lang, 'version': 1, 'text': (lane / path).read_text()}}})
            if lang == 'rust':
                # rust-analyzer loads Cargo metadata and primes analysis asynchronously.
                # An immediate request can return null during its initial workspace load.
                time.sleep(20)
            results = []
            for row in labels[lang]:
                site, target = anchor(lane, row['site']), anchor(lane, row['definition'])
                scip = scip_link(msg, site, target)
                uri = (lane / site['path']).as_uri()
                pos = {'line': site['line'], 'character': site['start']}
                found = server.request('textDocument/definition', {'textDocument': {'uri': uri}, 'position': pos}, 120)
                agrees, destinations = location_matches(found, target, lane)
                results.append({'site': row['site'], 'expected': row['definition'], 'kind': row['kind'],
                                'scip': scip, 'lsp_match': agrees, 'lsp_destinations': destinations})
                print(json.dumps({'language': lang, 'site': row['site'], 'scip_status': scip['status'], 'lsp_match': agrees, 'lsp_destinations': destinations}), flush=True)
            first = results[0]
            at = anchor(lane, first['expected'])
            references = server.request('textDocument/references', {'textDocument': {'uri': (lane / at['path']).as_uri()},
                'position': {'line': at['line'], 'character': at['start']}, 'context': {'includeDeclaration': False}}, 120)
            ref_found, ref_locations = location_matches(references, anchor(lane, first['site']), lane)
            print(json.dumps({'language': lang, 'source_set_sha256': SOURCE_DIGESTS[lang], 'scip_sha256': hashlib.sha256(index_bytes).hexdigest(),
                'server_command': commands[lang], 'server_capabilities': {'positionEncoding': response.get('capabilities', {}).get('positionEncoding')},
                'scip_matches': sum(r['scip']['status'] == 'match' for r in results), 'lsp_matches': sum(r['lsp_match'] for r in results),
                'site_denominator': len(results), 'reference_query_for_first_definition': {'found_labeled_site': ref_found,
                  'returned_count': len(references or []), 'samples': ref_locations}, 'results': results}), flush=True)
            if audit:
                print(json.dumps({'language': lang, 'cross_file_audit': audit_cross_file(server, msg, lane)}), flush=True)
        finally:
            server.close()


if __name__ == '__main__':
    args = sys.argv[2:]
    run(pathlib.Path(sys.argv[1]).resolve(), tuple(arg for arg in args if arg != '--audit-all') or LANGS, '--audit-all' in args)
