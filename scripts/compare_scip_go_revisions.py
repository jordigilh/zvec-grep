"""Compare two scip-go builds on identical staged Go bytes, per document.

Hashes the complete primary-site/role/symbol multiset in each document while
comparing definition enclosing ranges by primary-site identity. Binary SCIP
hashes can differ even on unchanged semantics, so they are not the comparator.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

if __package__:
    from scripts.scip_reference_probe import occ_coords
else:
    from scip_reference_probe import occ_coords


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def occurrence_key(occurrence) -> tuple:
    return (*occ_coords(occurrence), occurrence.symbol, occurrence.symbol_roles)


def enclosure(occurrence) -> str | None:
    kind = occurrence.WhichOneof("typed_enclosing_range")
    if kind:
        return f"{kind}:{getattr(occurrence, kind).SerializeToString(deterministic=True).hex()}"
    if occurrence.enclosing_range:
        return json.dumps(list(occurrence.enclosing_range), separators=(",", ":"))
    return None


def primary_digest(document) -> tuple[str, Counter]:
    sites = Counter(occurrence_key(occ) for occ in document.occurrences)
    digest = hashlib.sha256()
    for key, count in sorted(sites.items()):
        digest.update(json.dumps([*key, count], ensure_ascii=False, separators=(",", ":")).encode())
        digest.update(b"\n")
    return digest.hexdigest(), sites


def definitions(document) -> dict[tuple, Counter]:
    result = {}
    for occurrence in document.occurrences:
        if occurrence.symbol_roles & 1:
            key = occurrence_key(occurrence)
            result.setdefault(key, Counter())[enclosure(occurrence)] += 1
    return result


def compare(old_root: Path, new_root: Path, proto_root: Path) -> dict:
    sys.path.insert(0, str(proto_root.resolve(strict=True)))
    import scip_pb2

    old_path = old_root / "index.scip"
    new_path = new_root / "index.scip"
    old_bytes = old_path.read_bytes()
    new_bytes = new_path.read_bytes()
    old, new = scip_pb2.Index(), scip_pb2.Index()
    old.ParseFromString(old_bytes)
    new.ParseFromString(new_bytes)
    before = {doc.relative_path: doc for doc in old.documents}
    after = {doc.relative_path: doc for doc in new.documents}
    if len(before) != len(old.documents) or len(after) != len(new.documents) or set(before) != set(after):
        raise ValueError("producer document set changed or has duplicate paths")

    counts = Counter()
    samples = []
    for relative in sorted(before):
        old_source = (old_root / relative).read_bytes()
        new_source = (new_root / relative).read_bytes()
        if sha256(old_source) != sha256(new_source):
            raise ValueError(f"selected source changed across producer builds: {relative}")
        old_digest, old_sites = primary_digest(before[relative])
        new_digest, new_sites = primary_digest(after[relative])
        if old_digest != new_digest or old_sites != new_sites:
            counts["changed_primary_documents"] += 1
            if len(samples) < 5:
                samples.append({"path": relative, "reason": "primary site/role/symbol mismatch",
                                "missing_sites": sum((old_sites - new_sites).values()),
                                "added_sites": sum((new_sites - old_sites).values())})
        counts["documents"] += 1
        counts["source_bytes"] += len(old_source)
        counts["primary_occurrences"] += sum(old_sites.values())
        old_defs, new_defs = definitions(before[relative]), definitions(after[relative])
        for key in old_defs.keys() | new_defs.keys():
            old_values = old_defs.get(key, Counter())
            new_values = new_defs.get(key, Counter())
            old_present = sum(count for value, count in old_values.items() if value is not None)
            new_present = sum(count for value, count in new_values.items() if value is not None)
            counts["old_definition_enclosures"] += old_present
            counts["new_definition_enclosures"] += new_present
            if old_values == new_values:
                continue
            if old_present == 0 and new_present > 0:
                counts["new_enclosures"] += new_present
                if len(samples) < 5:
                    samples.append({"path": relative, "reason": "new declaration enclosure",
                                    "symbol": key[-2], "token_range": list(key[:4])})
            elif old_present > 0 and new_present == 0:
                counts["removed_enclosures"] += old_present
            else:
                counts["changed_existing_enclosures"] += 1

    return {
        "schema": "scip-go-revision-semantic-comparison-v1",
        "old_scip_sha256": sha256(old_bytes),
        "new_scip_sha256": sha256(new_bytes),
        "counts": dict(counts),
        "samples": samples,
        "primary_site_role_symbol_equal": counts["changed_primary_documents"] == 0,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old-root", type=Path, required=True)
    parser.add_argument("--new-root", type=Path, required=True)
    parser.add_argument("--proto-root", type=Path, required=True)
    args = parser.parse_args()
    result = compare(args.old_root.resolve(strict=True), args.new_root.resolve(strict=True), args.proto_root)
    print(json.dumps(result, indent=2))
    return 0 if result["primary_site_role_symbol_equal"] and result["counts"].get("removed_enclosures", 0) == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
