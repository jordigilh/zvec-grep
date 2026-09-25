"""Compare isolated scip-go v0.2.7 and upstream-fix artifacts on identical Go bytes.

python3 scripts/scip_go_upstream_comparison.py ROOT
ROOT/go contains baseline.scip and index.scip from two pinned binaries.
"""
import collections
import hashlib
import json
import pathlib
import sys
import urllib.parse

if __package__:
    from scripts.scip_spike import digest, range_bytes, selected
    from scripts.scip_reference_probe import SOURCE_DIGESTS
else:
    from scip_spike import digest, range_bytes, selected
    from scip_reference_probe import SOURCE_DIGESTS


def inspect(root, index, encoding, proto):
    lane = (root / 'go').resolve(strict=True)
    docs = {d.relative_path: d for d in index.documents}
    paths = selected('go')[2]
    project = urllib.parse.urlparse(index.metadata.project_root)
    if project.scheme != 'file' or pathlib.Path(urllib.parse.unquote(project.path)).resolve(strict=True) != lane:
        raise ValueError('project root mismatch')
    if sorted(docs) != paths or digest(lane, paths) != SOURCE_DIGESTS['go']:
        raise ValueError('source file selection/digest mismatch')
    counts = collections.Counter()
    sites = collections.Counter()
    definitions = {}
    for path, doc in docs.items():
        raw = (lane / path).read_bytes()
        for occ in doc.occurrences:
            counts['occurrences'] += 1
            try:
                a, b = range_bytes(raw.decode(), occ, encoding)
                if a >= b or b > len(raw):
                    raise ValueError('empty/out-of-bounds range')
            except ValueError:
                counts['invalid_primary'] += 1
                continue
            key = (path, a, b, occ.symbol, occ.symbol_roles)
            sites[key] += 1
            if occ.WhichOneof('typed_range'):
                counts['typed_primary'] += 1
            if occ.symbol_roles & 1:
                counts['definitions'] += 1
                enclosing = None
                if occ.enclosing_range or occ.WhichOneof('typed_enclosing_range'):
                    counts['definition_enclosing_present'] += 1
                    try:
                        start, end = range_bytes(raw.decode(), occ, encoding, enclosing=True)
                        if start <= a < b <= end:
                            enclosing = (start, end)
                            counts['definition_enclosing_valid'] += 1
                        else:
                            counts['definition_enclosing_not_containing'] += 1
                    except ValueError:
                        counts['definition_enclosing_invalid'] += 1
                definitions[key] = enclosing
            elif occ.symbol:
                counts['references'] += 1
    return counts, sites, definitions, {name: d.position_encoding for name, d in docs.items()}


def run(root):
    sys.path.insert(0, str(root))
    import scip_pb2
    lane = root / 'go'
    raw_before = (lane / 'baseline.scip').read_bytes()
    raw_after = (lane / 'index.scip').read_bytes()
    before = scip_pb2.Index()
    after = scip_pb2.Index()
    before.ParseFromString(raw_before)
    after.ParseFromString(raw_after)
    if before.metadata.tool_info.name != 'scip-go' or after.metadata.tool_info.name != 'scip-go':
        raise ValueError('unexpected producers')
    prior, prior_sites, prior_defs, prior_enc = inspect(root, before, 1, scip_pb2)
    current, current_sites, current_defs, current_enc = inspect(root, after, 1, scip_pb2)
    if current_enc != {name: 1 for name in current_enc}:
        raise ValueError('patched Go producer did not declare UTF-8')
    new_enclosings = [(path, a, b, symbol, body) for (path, a, b, symbol, role), body in current_defs.items()
                      if body and not prior_defs.get((path, a, b, symbol, role))]
    changed_prior = [(key, body, current_defs.get(key)) for key, body in prior_defs.items()
                     if body and current_defs.get(key) != body]
    print(json.dumps({'language': 'go', 'root': str(lane.resolve(strict=True)),
        'source_set_sha256': digest(lane, selected('go')[2]),
        'baseline_sha256': hashlib.sha256(raw_before).hexdigest(),
        'candidate_sha256': hashlib.sha256(raw_after).hexdigest(),
        'baseline_bytes': len(raw_before), 'candidate_bytes': len(raw_after),
        'baseline_encoding': prior_enc, 'candidate_encoding': current_enc,
        'baseline_counts': prior, 'candidate_counts': current,
        'primary_site_role_symbol_multiset_equal': prior_sites == current_sites,
        'primary_site_missing_from_candidate': sum((prior_sites - current_sites).values()),
        'primary_site_added_by_candidate': sum((current_sites - prior_sites).values()),
        'new_valid_enclosings': len(new_enclosings),
        'changed_existing_enclosing_count': len(changed_prior),
        'new_enclosing_examples': new_enclosings[:8]}), flush=True)


if __name__ == '__main__':
    run(pathlib.Path(sys.argv[1]).resolve(strict=True))
