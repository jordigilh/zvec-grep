#!/usr/bin/env python3
"""Adjudicate upstream zg CLI-exported edges against compiler truth (both Go fixtures).
Generalized from graph-precision-shootout-output/adjudicate_shootout.py (same method:
endpoint identity = fixture-relative file + manifest symbol; bare names resolved
per-file via exact match, else unique 'Receiver.name' suffix).
Usage: adjudicate_upstream.py <tool> <edges.json> <truth.json> <manifest.json> <out.json>
"""
import json, sys
from collections import Counter, defaultdict

tool, edge_path, truth_path, manifest_path, out_path = sys.argv[1:6]

truth = json.load(open(truth_path))
manifest = json.load(open(manifest_path))
units = manifest['units']

comp_pairs = set()
comp_pair_sites = defaultdict(list)
comp_nodes = {}
for e in truth['edges']:
    for n in (e['caller'], e['callee']):
        if n['symbol']:
            comp_nodes[(n['span']['file'], n['symbol'])] = n
    k = ((e['caller']['span']['file'], e['caller']['symbol']),
         (e['callee']['span']['file'], e['callee']['symbol']))
    comp_pairs.add(k)
    comp_pair_sites[k].append(e)

manifest_keys = {(u['path'], u['symbol']) for u in units}
by_path = defaultdict(list)
for u in units:
    by_path[u['path']].append(u['symbol'])

def norm_cli_symbol(sym):
    """CLI prints C++-style 'Recv::Method'; manifest uses 'Recv.Method'."""
    return sym.replace('::', '.') if '::' in sym else sym

def resolve_symbol(file, symbol):
    if symbol is None:
        return None
    symbol = norm_cli_symbol(symbol)
    if (file, symbol) in manifest_keys:
        return symbol
    syms = by_path.get(file, [])
    if symbol in syms:
        return symbol
    suffix = [s for s in syms if s.endswith('.' + symbol)]
    if len(suffix) == 1:
        return suffix[0]
    return symbol

def entity_status(key):
    if key is None:
        return 'unmapped'
    if key in comp_nodes:
        return 'callable'
    if key in manifest_keys:
        return 'type_or_noncallable'
    return 'unknown'

doc = json.load(open(edge_path))
edges = doc['edges']

adjudication = []
tool_pairs = set()
for ed in edges:
    s, t = ed['source'], ed['target']
    sk = (s['file'], resolve_symbol(s['file'], s.get('symbol'))) if s.get('file') else None
    tk = (t['file'], resolve_symbol(t['file'], t.get('symbol'))) if t.get('file') else None
    s_stat, t_stat = entity_status(sk), entity_status(tk)
    rec = {
        'source': {'key': list(sk) if sk else None, 'status': s_stat,
                   'raw_symbol': s.get('symbol'), 'raw_file': s.get('file')},
        'target': {'key': list(tk) if tk else None, 'status': t_stat,
                   'raw_symbol': t.get('symbol'), 'raw_file': t.get('file')},
        'resolved': ed.get('resolved', True),
        'raw': ed.get('raw'),
    }
    pair, rpair = (sk, tk), (tk, sk)
    if pair in comp_pairs:
        rec['verdict'] = 'CONFIRMED'
        rec['direction'] = 'correct'
        rec['compiler_evidence'] = [e['call_site'] for e in comp_pair_sites[pair]]
    elif rpair in comp_pairs:
        rec['verdict'] = 'REFUTED'
        rec['direction'] = 'flipped'
        rec['reason'] = f'compiler edge exists in reverse: {tk[1]} calls {sk[1]}'
    elif s_stat in ('unmapped', 'unknown') or t_stat in ('unmapped', 'unknown'):
        rec['verdict'] = 'UNRESOLVED'
        rec['direction'] = None
        rec['reason'] = 'endpoint could not be mapped to a compiler/manifest entity'
    else:
        rec['verdict'] = 'REFUTED'
        rec['direction'] = 'n/a'
        if t_stat == 'type_or_noncallable':
            rec['reason'] = 'target is a type declaration, not a callable'
            rec['failure_mode'] = 'type_mention'
        elif s_stat == 'type_or_noncallable':
            rec['reason'] = 'source is a type declaration, not a callable'
            rec['failure_mode'] = 'type_mention'
        else:
            rec['reason'] = ('both endpoints are fixture callables but the compiler '
                             'finds no call in either direction')
            rec['failure_mode'] = 'no_call_in_either_direction'
    if rec['verdict'] == 'REFUTED' and sk and tk and (
            sk[1] == tk[1] or (ed.get('raw') or {}).get('self_loop')):
        rec['failure_mode'] = 'same_name_collision'
    adjudication.append(rec)
    if sk and tk:
        tool_pairs.add((sk, tk))

vc = Counter(r['verdict'] for r in adjudication)
flips = sum(1 for r in adjudication if r.get('direction') == 'flipped')
fm = Counter(r.get('failure_mode') for r in adjudication if r['verdict'] == 'REFUTED')

present = [p for p in comp_pairs if p in tool_pairs]
rev_present = [p for p in comp_pairs if (p[1], p[0]) in tool_pairs and p not in tool_pairs]
missed = [p for p in comp_pairs if p not in tool_pairs and (p[1], p[0]) not in tool_pairs]

out = {
    'schema': 'graph-shootout-adjudication-v1',
    'tool': tool,
    'input': edge_path,
    'truth': truth_path,
    'edge_count': len(adjudication),
    'unique_pairs': len(tool_pairs),
    'verdicts': dict(vc),
    'failure_modes': {k: v for k, v in fm.items() if k},
    'direction_flips': flips,
    'precision': (vc['CONFIRMED'] / len(adjudication)) if adjudication else None,
    'recall': len(present) / len(comp_pairs),
    'compiler_pairs_total': len(comp_pairs),
    'compiler_pairs_present': len(present),
    'compiler_pairs_missed': [
        {'caller': a[1], 'callee': b[1], 'caller_file': a[0], 'callee_file': b[0]}
        for a, b in missed],
    'edges': adjudication,
}
json.dump(out, open(out_path, 'w'), indent=1)
print(tool, 'edges:', len(adjudication), 'verdicts:', dict(vc), 'flips:', flips,
      'failure_modes:', dict(fm), 'recall:', out['recall'])
for m in missed:
    print('  MISSED:', m[0][1], '->', m[1][1])
