"""Strict shadow-only join of read-only syntax Code IR v1 and LSP-only facts.

python3 scripts/lsp_ir_shadow_join.py ROOT [typescript|python|rust|go ...]
Never mutates the active design checkout or the input snapshots. An emitted
type_resolved fact is a candidate in an offline, validated shadow artifact,
not a production resolution or ranking decision.
"""
import hashlib
import copy
import json
import pathlib
import sys
from collections import Counter

from jsonschema import Draft202012Validator

if __package__:
    from scripts.scip_spike import LANGS, digest, selected, snapshot_key
    from scripts.scip_reference_probe import LABELS, SOURCE_DIGESTS, anchor
    from scripts.lsp_code_ir_spike import CONFIG
else:
    from scip_spike import LANGS, digest, selected, snapshot_key
    from scip_reference_probe import LABELS, SOURCE_DIGESTS, anchor
    from lsp_code_ir_spike import CONFIG

DESIGN_SCHEMA = pathlib.Path('/Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design/schemas/code-ir-v1.schema.json')
KIND = {2: 'module', 3: 'module', 4: 'module', 5: 'type', 6: 'method', 7: 'value',
        8: 'value', 9: 'method', 10: 'type', 11: 'type', 12: 'function',
        13: 'value', 14: 'value', 15: 'value', 16: 'value', 17: 'value',
        19: 'type', 20: 'value', 21: 'value', 22: 'value', 23: 'type', 24: 'value', 25: 'value', 26: 'value'}


def hash_bytes(raw):
    return hashlib.sha256(raw).hexdigest()


def inspect_snapshot(lane, lang, lsp_record, ir_record):
    """Fail closed on any root, selected file, source, config or identity mismatch."""
    lane = lane.resolve(strict=True)
    lsp = lsp_record['provenance']
    ir = ir_record['snapshot']
    paths = selected(lang)[2]
    if (pathlib.Path(ir_record['provenance']['staged_root']).resolve(strict=True) != lane or
        pathlib.Path(lsp['staged_root']).resolve(strict=True) != lane or
        sorted(lsp['selected_paths']) != paths or
        sorted(f['relative_path'] for f in ir['files']) != paths or
        ir['root_set'] != [f'{lang}-workflow-discovery-v1'] or
        ir['schema'] != 'zvec-grep.code-ir' or ir['schema_version'] != 1):
        raise ValueError('cross-root/selection/schema mismatch')
    files_hash = digest(lane, paths)
    config_hash = hash_bytes((lane / CONFIG[lang]).read_bytes())
    if (files_hash != lsp['source_set_sha256'] or files_hash != SOURCE_DIGESTS[lang] or
        config_hash != lsp['config_sha256']):
        raise ValueError('stale selected source/config snapshot')
    actual_key = snapshot_key(lane, paths, lsp.get('semantic_source', 'lsp'), lsp['producer'], config_hash,
                              f'Engram fixture-v1 staged copy; root={lane}; source-set={files_hash}')
    if lsp['snapshot_key'] != actual_key or lsp['lsp_position_encoding'] not in ('utf-8', 'utf-16', 'utf-32'):
        raise ValueError('invalid LSP snapshot/encoding provenance')
    by_path = {f['relative_path']: f for f in ir['files']}
    if len(by_path) != len(paths):
        raise ValueError('duplicate IR file')
    for name in paths:
        file = by_path[name]
        data = (lane / name).read_bytes()
        if (file['sha256'] != hash_bytes(data) or file['byte_length'] != len(data) or
            file['extraction']['status'] != 'complete' or file['language'] != lang):
            raise ValueError(f'stale/partial IR file {name}')
    return ir, by_path


def source_ref(file, raw, a, b):
    if not 0 <= a < b <= len(raw):
        raise ValueError('invalid byte range')
    if (raw[a] & 0xC0) == 0x80 or (b < len(raw) and (raw[b] & 0xC0) == 0x80):
        raise ValueError('split UTF-8 code point')
    raw[a:b].decode('utf-8')
    def loc(offset):
        prefix = raw[:offset]
        return {'line': prefix.count(b'\n') + 1,
                'column_byte': len(prefix) - (prefix.rfind(b'\n') + 1)}
    return {'file_id': file['file_id'], 'sha256': file['sha256'], 'start_byte': a,
            'end_byte': b, 'start': loc(a), 'end': loc(b)}


def checked_evidence(evidence, root, files):
    path = evidence['path']
    if path not in files:
        raise ValueError('LSP evidence outside IR selected files')
    raw = (root / path).read_bytes()
    file = files[path]
    if hash_bytes(raw) != file['sha256'] or evidence['source_sha256'] != file['sha256']:
        raise ValueError('LSP source digest does not match IR file')
    ref = source_ref(file, raw, evidence['start_byte'], evidence['end_byte'])
    if (raw[ref['start_byte']:ref['end_byte']].decode('utf-8') != evidence['slice'] or
        ref['start']['line'] != evidence['line'] or
        ref['start']['column_byte'] != evidence['byte_column']):
        raise ValueError('LSP slice/line/byte column mismatch')
    return ref


def strict_symbol_binding(symbol, units, root, files):
    """No name-only or approximate body range joins."""
    token = symbol['token']
    ref = checked_evidence(token, root, files)
    expected_kind = KIND.get(symbol['kind'])
    candidates = [unit for unit in units if unit['source']['file_id'] == ref['file_id']
                  and unit.get('name') == token['slice'] and unit['kind'] == expected_kind
                  and unit['source']['start_byte'] == symbol['body']['start_byte']
                  and unit['source']['end_byte'] == symbol['body']['end_byte']
                  and unit['source']['start_byte'] <= ref['start_byte']
                  and ref['end_byte'] <= unit['source']['end_byte']]
    if len(candidates) == 1:
        return candidates[0], 'strict'
    if len(candidates) > 1:
        return None, 'ambiguous'
    same_token = [unit for unit in units if unit['source']['file_id'] == ref['file_id']
                  and unit.get('name') == token['slice']
                  and unit['source']['start_byte'] <= ref['start_byte']
                  and ref['end_byte'] <= unit['source']['end_byte']]
    return None, ('body_or_kind_mismatch' if same_token else 'missing_syntax_unit')


def smallest_subject(site, units):
    matches = [unit for unit in units if unit['source']['file_id'] == site['file_id']
               and unit['source']['start_byte'] <= site['start_byte']
               and site['end_byte'] <= unit['source']['end_byte']
               and unit['kind'] != 'opaque']
    if not matches:
        return None, 'uncontained'
    matches.sort(key=lambda u: (u['source']['end_byte'] - u['source']['start_byte'],
                                u['source']['start_byte'], u['id']))
    shortest = matches[0]['source']['end_byte'] - matches[0]['source']['start_byte']
    if sum(u['source']['end_byte'] - u['source']['start_byte'] == shortest for u in matches) != 1:
        return None, 'ambiguous_subject'
    return matches[0], 'file' if matches[0]['kind'] == 'file' else 'unit'


def shadow_join(lane, lang, lsp_record, ir_record):
    ir, files = inspect_snapshot(lane, lang, lsp_record, ir_record)
    units = ir['units']
    counts = {'lsp_symbols': len(lsp_record['symbols']), 'strict_symbol_bindings': 0,
              'symbol_body_or_kind_mismatch': 0, 'symbol_missing_syntax_unit': 0,
              'symbol_ambiguous': 0, 'references': len(lsp_record['references']),
              'strict_references': 0, 'subject_file_fallback': 0,
              'subject_uncontained': 0, 'subject_ambiguous': 0,
              'unbound_target': 0, 'evidence_rejected': 0}
    bindings = {}
    for symbol in lsp_record['symbols']:
        unit, status = strict_symbol_binding(symbol, units, lane, files)
        if unit:
            counts['strict_symbol_bindings'] += 1
            token = symbol['token']
            key = (token['path'], token['start_byte'], token['end_byte'])
            bindings.setdefault(key, set()).add(unit['id'])
        else:
            counts['symbol_' + status if status != 'body_or_kind_mismatch' else 'symbol_body_or_kind_mismatch'] += 1
    unit_ids = {u['id']: u for u in units}
    for key in list(bindings):
        if len(bindings[key]) != 1:
            counts['symbol_ambiguous'] += 1
            del bindings[key]
    new_facts = []
    for item in lsp_record['references']:
        try:
            site = checked_evidence(item['site'], lane, files)
            target = checked_evidence(item['target'], lane, files)
        except (ValueError, UnicodeDecodeError):
            counts['evidence_rejected'] += 1
            continue
        target_token = item['target']
        key = (target_token['path'], target['start_byte'], target['end_byte'])
        target_units = bindings.get(key)
        if not target_units:
            counts['unbound_target'] += 1
            continue
        subject, status = smallest_subject(site, units)
        if subject is None:
            counts['subject_ambiguous' if status == 'ambiguous_subject' else 'subject_uncontained'] += 1
            continue
        if status == 'file':
            counts['subject_file_fallback'] += 1
        target_unit = unit_ids[next(iter(target_units))]
        identity = [ir['snapshot_id'], subject['id'], target_unit['id'],
                    site['file_id'], site['start_byte'], site['end_byte'], 'references']
        new_facts.append({'id': hash_bytes(json.dumps(identity, separators=(',', ':')).encode()),
                          'kind': 'references', 'subject_id': subject['id'],
                          'object_id': target_unit['id'], 'target_spelling': item['site']['slice'],
                          'site': site, 'status': 'type_resolved',
                          'provenance': {'frontend': ('scip-shadow-v1' if lsp_record['provenance'].get('semantic_source', '').startswith('scip')
                                                       else 'lsp-shadow-v1'),
                                         'resolver': lang,
                                         'resolver_version': lsp_record['provenance']['producer'],
                                         'method': ('SCIP symbol occurrence + strict declaration enclosing-range/kind join'
                                                    if lsp_record['provenance'].get('semantic_source', '').startswith('scip')
                                                    else 'find-references + strict declaration-byte-span/kind join')}})
        counts['strict_references'] += 1
    result = {**ir, 'facts': ir['facts'] + new_facts}
    schema = json.loads(DESIGN_SCHEMA.read_text())
    Draft202012Validator(schema).validate(result)
    return result, counts


def run(root, languages):
    for lang in languages:
        lane = (root / lang).resolve(strict=True)
        lsp_bytes = (root / f'lsp-full-{lang}.json').read_bytes()
        ir_bytes = (root / f'syntax-ir-{lang}.json').read_bytes()
        snapshot, counts = shadow_join(lane, lang, json.loads(lsp_bytes), json.loads(ir_bytes))
        output = root / f'shadow-ir-{lang}.json'
        payload = (json.dumps({'snapshot': snapshot, 'join_counts': counts,
                               'lsp_artifact_sha256': hash_bytes(lsp_bytes),
                               'syntax_artifact_sha256': hash_bytes(ir_bytes)},
                              sort_keys=True, separators=(',', ':')) + '\n').encode()
        with output.open('xb') as stream:
            stream.write(payload)
        print(json.dumps({'language': lang, **counts, 'schema_valid': True,
                          'syntax_snapshot_id': snapshot['snapshot_id'], 'shadow_facts': len(snapshot['facts']),
                          'artifact': str(output), 'artifact_sha256': hash_bytes(payload)}), flush=True)


def inspect(root, lang):
    lane = (root / lang).resolve(strict=True)
    lsp_record = json.loads((root / f'lsp-full-{lang}.json').read_text())
    ir_record = json.loads((root / f'syntax-ir-{lang}.json').read_text())
    ir, files = inspect_snapshot(lane, lang, lsp_record, ir_record)
    examples = {}
    for symbol in lsp_record['symbols']:
        _, status = strict_symbol_binding(symbol, ir['units'], lane, files)
        if status == 'strict' or len(examples.get(status, [])) >= 8:
            continue
        path = symbol['token']['path']
        name = symbol['token']['slice']
        alternatives = [{'kind': u['kind'], 'range': [u['source']['start_byte'], u['source']['end_byte']], 'name': u.get('name')}
                        for u in ir['units'] if u['source']['file_id'] == files[path]['file_id'] and u.get('name') == name]
        examples.setdefault(status, []).append({'name': symbol['name'], 'kind': symbol['kind'], 'path': path,
            'token': name, 'lsp_body': symbol['body'], 'syntax_candidates': alternatives[:3]})
    print(json.dumps({'language': lang, 'mismatch_examples': examples}), flush=True)


def inspect_unmatched(root, lang):
    lane = (root / lang).resolve(strict=True)
    lsp = json.loads((root / f'lsp-full-{lang}.json').read_text())
    ir_record = json.loads((root / f'syntax-ir-{lang}.json').read_text())
    ir, files = inspect_snapshot(lane, lang, lsp, ir_record)
    symbols = {}
    by_kind = Counter()
    owner_symbols = Counter()
    for symbol in lsp['symbols']:
        _, status = strict_symbol_binding(symbol, ir['units'], lane, files)
        token = symbol['token']
        symbols[(token['path'], token['start_byte'], token['end_byte'])] = (symbol, status)
        if status != 'strict':
            by_kind[(status, symbol['kind'])] += 1
            ref = checked_evidence(token, lane, files)
            owner, _ = smallest_subject(ref, ir['units'])
            owner_symbols[owner['kind'] if owner else 'none'] += 1
    target_counts = Counter()
    owner_refs = Counter()
    examples = {}
    for fact in lsp['references']:
        target = fact['target']
        entry = symbols.get((target['path'], target['start_byte'], target['end_byte']))
        if not entry or entry[1] == 'strict':
            continue
        symbol, status = entry
        key = (status, symbol['kind'])
        target_counts[key] += 1
        ref = checked_evidence(target, lane, files)
        owner, _ = smallest_subject(ref, ir['units'])
        owner_refs[owner['kind'] if owner else 'none'] += 1
        if len(examples.get(str(key), [])) < 4:
            examples.setdefault(str(key), []).append({'name': symbol['name'], 'token': target['slice'],
                'path': target['path'], 'reference': fact['site']['slice']})
    print(json.dumps({'language': lang, 'unmatched_symbols_by_status_kind': {str(k): v for k,v in by_kind.items()},
        'unbound_references_by_target_status_kind': {str(k): v for k,v in target_counts.items()},
        'unmatched_target_owner_unit_kinds': dict(owner_symbols),
        'unbound_reference_target_owner_unit_kinds': dict(owner_refs),
        'samples': examples}), flush=True)


def check_labels(root, languages, artifact_prefix='shadow-ir'):
    labels = json.loads(LABELS.read_text())
    for lang in languages:
        lane = root / lang
        joined = json.loads((root / f'{artifact_prefix}-{lang}.json').read_text())['snapshot']
        files = {f['file_id']: f['relative_path'] for f in joined['files']}
        units = {u['id']: u for u in joined['units']}
        matches = 0
        missing = []
        for row in labels[lang]:
            site, target = anchor(lane, row['site']), anchor(lane, row['definition'])
            def absolute(item):
                lines = (lane / item['path']).read_bytes().splitlines(keepends=True)
                return sum(map(len, lines[:item['line']])) + item['start']
            facts = [f for f in joined['facts'] if f['kind'] == 'references'
                     and files[f['site']['file_id']] == site['path'] and f['site']['start_byte'] == absolute(site)]
            good = any(f.get('object_id') and files[units[f['object_id']]['source']['file_id']] == target['path']
                       and units[f['object_id']]['source']['start_byte'] <= absolute(target)
                       < units[f['object_id']]['source']['end_byte'] for f in facts)
            matches += good
            if not good:
                missing.append({'site': row['site'], 'expected': row['definition'],
                                'actual_target_ids': [f.get('object_id') for f in facts]})
        print(json.dumps({'language': lang, 'strict_join_authored_matches': matches,
                          'denominator': len(labels[lang]), 'misses': missing}), flush=True)


def dirty_target_probe(root):
    """Edit only a staged Go cross-file target after the LSP artifact exists."""
    lane = (root / 'go').resolve(strict=True)
    paths = selected('go')[2]
    before = digest(lane, paths)
    caller = lane / 'internal/selection/validator.go'
    target = lane / 'internal/discovery/state.go'
    caller_hash = hash_bytes(caller.read_bytes())
    original = target.read_bytes()
    assert b'func (s *WorkflowState) Contains(' in original
    target.write_bytes(original.replace(b'func (s *WorkflowState) Contains(',
                                        b'func (s *WorkflowState) ContainsDirty(', 1))
    after = digest(lane, paths)
    assert caller_hash == hash_bytes(caller.read_bytes()) and before != after
    print(json.dumps({'probe': 'staged changed cross-file Go target',
                      'old_file_set': before, 'new_file_set': after,
                      'caller_sha256_unchanged': caller_hash,
                      'target_old_sha256': hash_bytes(original),
                      'target_new_sha256': hash_bytes(target.read_bytes())}))


def tamper_probe(root, lang):
    """Exercise fail-closed behavior on actual decoded LSP/syntax artifacts."""
    lane = (root / lang).resolve(strict=True)
    lsp = json.loads((root / f'lsp-full-{lang}.json').read_text())
    ir = json.loads((root / f'syntax-ir-{lang}.json').read_text())
    joined, baseline = shadow_join(lane, lang, lsp, ir)
    cross_file = next(item for item in lsp['references'] if item['site']['path'] != item['target']['path'])
    target_path = cross_file['target']['path']
    failures = {}
    for name, tamper in {
        'different_staged_root': lambda x, y: x['provenance'].__setitem__('staged_root', '/elsewhere'),
        'changed_config': lambda x, y: x['provenance'].__setitem__('config_sha256', '0' * 64),
        'changed_cross_file_target_hash': lambda x, y: next(f for f in y['snapshot']['files'] if f['relative_path'] == target_path).__setitem__('sha256', '0' * 64),
    }.items():
        x, y = copy.deepcopy(lsp), copy.deepcopy(ir)
        tamper(x, y)
        try:
            shadow_join(lane, lang, x, y)
            failures[name] = 'accepted (BUG)'
        except (ValueError, OSError) as exc:
            failures[name] = str(exc)
    forged = copy.deepcopy(lsp)
    first = next(f for f in joined['facts'] if f['provenance']['frontend'] == 'lsp-shadow-v1')
    file_path = next(f['relative_path'] for f in joined['files'] if f['file_id'] == first['site']['file_id'])
    matching = next(item for item in forged['references'] if item['site']['path'] == file_path
                    and item['site']['start_byte'] == first['site']['start_byte'])
    matching['site']['source_sha256'] = '0' * 64
    _, after = shadow_join(lane, lang, forged, ir)
    failures['forged_site_digest'] = {'rejected_evidence': after['evidence_rejected'],
                                       'new_facts_delta': baseline['strict_references'] - after['strict_references']}
    print(json.dumps({'language': lang, 'tamper_results': failures}), flush=True)


if __name__ == '__main__':
    root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    if '--inspect' in sys.argv[2:]:
        inspect(root, sys.argv[3])
    elif '--unmatched' in sys.argv[2:]:
        inspect_unmatched(root, sys.argv[3])
    elif '--check-labels' in sys.argv[2:]:
        options = tuple(arg for arg in sys.argv[2:] if arg not in ('--check-labels', '--scip', '--compat')) or LANGS
        prefix = ('compat-v2-shadow-ir' if '--compat' in sys.argv[2:] else
                  'scip-shadow-ir' if '--scip' in sys.argv[2:] else 'shadow-ir')
        check_labels(root, options, prefix)
    elif '--dirty-target-probe' in sys.argv[2:]:
        dirty_target_probe(root)
    elif '--tamper-probe' in sys.argv[2:]:
        tamper_probe(root, sys.argv[3])
    else:
        run(root, tuple(sys.argv[2:]) or LANGS)
