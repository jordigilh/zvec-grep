"""Compare same-source Code IR shadow facts across two SCIP producer revisions.

Run only after each artifact has passed its own pinned source/index/IR checks.
This comparison uses IDs and the complete fact shapes, not just aggregate counts.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def canonical_facts(snapshot: dict) -> dict[str, bytes]:
    facts = {}
    for fact in snapshot["facts"]:
        if fact["id"] in facts:
            raise ValueError("duplicate IR fact ID")
        facts[fact["id"]] = json.dumps(fact, sort_keys=True, separators=(",", ":")).encode()
    return facts


def compare(old_path: Path, new_path: Path) -> dict:
    old = json.loads(old_path.read_text())["snapshot"]
    new = json.loads(new_path.read_text())["snapshot"]
    if old["schema"] != "zvec-grep.code-ir" or new["schema"] != old["schema"]:
        raise ValueError("unsupported or mismatched IR schema")
    if old["snapshot_id"] != new["snapshot_id"]:
        raise ValueError("cannot compare different source/frontends as one IR snapshot")
    old_units = {unit["id"]: json.dumps(unit, sort_keys=True, separators=(",", ":")) for unit in old["units"]}
    new_units = {unit["id"]: json.dumps(unit, sort_keys=True, separators=(",", ":")) for unit in new["units"]}
    if old_units != new_units:
        raise ValueError("producer comparison changed canonical syntax units")
    before, after = canonical_facts(old), canonical_facts(new)
    added = sorted(after.keys() - before.keys())
    removed = sorted(before.keys() - after.keys())
    changed = sorted(key for key in before.keys() & after.keys() if before[key] != after[key])
    reference_count = lambda facts: sum(json.loads(raw)["kind"] == "references" for raw in facts.values())
    checksum = lambda facts: hashlib.sha256(b"\n".join(facts[key] for key in sorted(facts))).hexdigest()
    return {
        "schema": "scip-shadow-factset-comparison-v1",
        "snapshot_id": old["snapshot_id"],
        "syntax_units": len(old_units),
        "old_facts": len(before),
        "new_facts": len(after),
        "old_references": reference_count(before),
        "new_references": reference_count(after),
        "old_factset_sha256": checksum(before),
        "new_factset_sha256": checksum(after),
        "added": len(added), "removed": len(removed), "changed": len(changed),
        "examples": {"added": added[:3], "removed": removed[:3], "changed": changed[:3]},
        "equal": not (added or removed or changed),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("old", type=Path)
    parser.add_argument("new", type=Path)
    args = parser.parse_args()
    result = compare(args.old, args.new)
    print(json.dumps(result, indent=2))
    return 0 if result["equal"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
