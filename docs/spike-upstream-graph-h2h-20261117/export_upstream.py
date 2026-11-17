#!/usr/bin/env python3
"""Export upstream zg graph edges via `zg --callers/--callees` per symbol.

The upstream graph DB stores endpoints as opaque zvec entity ids (idmap
rocksdb), so the full edge set is not directly exportable from SQLite; we
reconstruct it by querying every manifest symbol and parsing the CLI text
output (the limitation issue #43 anticipated). Raw SQLite rows are dumped
separately for substrate analysis (provenance/status/candidates).

Usage: export_upstream.py <zg-binary> <fixture-dir> <manifest.json> <out.json>
"""
import json, subprocess, sys, re

zg, root, manifest_path, out_path = sys.argv[1:5]
manifest = json.load(open(manifest_path))
units = manifest['units']

sym_re = re.compile(r'^(?P<sym>.*?)\s{2}(?P<file>\S+?):\d+$')
call_re = re.compile(r'^call at (?P<file>\S+?):\d+:\d+$')

def query(flag, symbol):
    r = subprocess.run([zg, flag, symbol, root, '--mode', 'direct', '--limit', '50',
                        '--color', 'never'], capture_output=True, text=True)
    return r.stdout

edges = {}   # (src_key, dst_key) -> edge
def add(src_sym, src_file, dst_sym, dst_file, site_file=None, raw=None):
    k = (src_sym, src_file, dst_sym, dst_file)
    e = edges.setdefault(k, {
        'source': {'symbol': src_sym, 'file': src_file},
        'target': {'symbol': dst_sym, 'file': dst_file},
        'resolved': True, 'call_sites': [], 'raw': raw or {}})
    if site_file:
        e['call_sites'].append(site_file)

for u in units:
    sym = u['symbol']
    for flag in ('--callees', '--callers'):
        out = query(flag, sym)
        cur_def = None      # (sym, file) of the definition block
        cur_related = None  # (sym, file) of the related definition
        for line in out.splitlines():
            line = line.rstrip()
            if not line.strip():
                cur_def = cur_related = None
                continue
            stripped = line.strip()
            m = sym_re.match(line)
            if m and not line.startswith('  '):
                cur_def = (m['sym'], m['file']); cur_related = None
                continue
            if cur_def is None:
                continue
            if stripped in ('Callees:', 'Callers:', 'No resolved call sites.'):
                continue
            m2 = sym_re.match(stripped)
            if m2:
                cur_related = (m2['sym'], m2['file'])
                continue
            m3 = call_re.match(stripped)
            if m3 and cur_related:
                if flag == '--callees':
                    add(cur_def[0], cur_def[1], cur_related[0], cur_related[1], m3['file'])
                else:
                    add(cur_related[0], cur_related[1], cur_def[0], cur_def[1], m3['file'])

doc = {'schema': 'upstream-zg-cli-edges-v1', 'tool': 'upstream zg 0.3.1 (7504720) --callers/--callees',
       'fixture_root': root, 'edges': list(edges.values())}
json.dump(doc, open(out_path, 'w'), indent=1)
print(f'{len(edges)} unique directed edges (with files) from {len(units)} manifest symbols')
