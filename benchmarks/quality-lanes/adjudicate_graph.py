#!/usr/bin/env python3
"""Graph quality lane: adjudicate the native zg codegraph's call edges against
committed compiler truth (go/packages + SSA + RTA, schema go-graph-truth-v1).

Method ported from the 2026-11-16 graph precision shootout
(adjudicate_shootout.py + normalize_zvec.py): the fixture's source files are
copied into a fresh work root, `zg --graph` builds a native codegraph snapshot
there, the `calls` edges are normalized to (fixture-relative file, bare
symbol) endpoints, and each edge is adjudicated against the committed
compiler truth. Endpoint identity is (fixture-relative file, manifest
symbol); bare names are resolved per-file via exact match, else a unique
`Receiver.name` suffix match.

The compiler-truth JSON files are committed beside the fixtures, so this lane
needs no Go toolchain at CI time; tools/go-graph-truth regenerates them.

Usage:
  python3 adjudicate_graph.py --fixture <fixture-dir> --binary <zg> \
      --work-root <dir> --output <file>
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from replay_normalization import copy_fixture_sources  # noqa: E402

CODEGRAPH_ARTIFACT = Path(".zvec-grep") / "codegraph-v2.json.zst"


def load_codegraph(root: Path) -> dict:
    artifact = root / CODEGRAPH_ARTIFACT
    if not artifact.is_file():
        raise SystemExit(f"zg --graph did not produce {artifact}")
    try:
        import zstandard  # type: ignore

        with artifact.open("rb") as handle:
            with zstandard.ZstdDecompressor().stream_reader(handle) as reader:
                return json.loads(reader.read())
    except ImportError:
        completed = subprocess.run(
            ["zstd", "-dc", str(artifact)], capture_output=True, check=False
        )
        if completed.returncode:
            raise SystemExit(
                f"zstd failed on {artifact}: {completed.stderr.decode(errors='replace')[:2000]}"
            )
        return json.loads(completed.stdout)


def normalize_calls_edges(codegraph: dict) -> list[dict]:
    """codegraph-v2 `calls` edges to the adjudication edge format."""
    nodes = {n["id"]: n for n in codegraph["nodes"]}
    edges = []
    for e in codegraph["edges"]:
        if e["kind"] != "calls":
            continue
        s = nodes.get(e["source"])
        if s is None:
            continue
        if e.get("target"):
            t = nodes.get(e["target"])
            if t is None:
                continue
            rec = {"source": {"file": s["path"], "symbol": s["name"]},
                   "target": {"file": t["path"], "symbol": t["name"]},
                   "resolved": True,
                   "raw": {"source_qualified": s.get("qualified_name"),
                           "target_qualified": t.get("qualified_name"),
                           "target_name": e.get("target_name"),
                           "resolution": e.get("resolution"),
                           "range": e.get("range")}}
        else:
            rec = {"source": {"file": s["path"], "symbol": s["name"]},
                   "target": {"file": None, "symbol": e.get("target_name")},
                   "resolved": False,
                   "raw": {"source_qualified": s.get("qualified_name"),
                           "target_name": e.get("target_name"),
                           "resolution": e.get("resolution"),
                           "range": e.get("range")}}
        edges.append(rec)
    return edges


def adjudicate(edges: list[dict], truth: dict, manifest: dict) -> dict:
    units = manifest["units"]

    # compiler truth pairs keyed by (file, symbol)
    comp_pairs = set()
    comp_pair_sites = defaultdict(list)
    comp_nodes = {}
    for e in truth["edges"]:
        for n in (e["caller"], e["callee"]):
            if n["symbol"]:
                comp_nodes[(n["span"]["file"], n["symbol"])] = n
        k = ((e["caller"]["span"]["file"], e["caller"]["symbol"]),
             (e["callee"]["span"]["file"], e["callee"]["symbol"]))
        comp_pairs.add(k)
        comp_pair_sites[k].append(e)

    manifest_keys = {(u["path"], u["symbol"]) for u in units}
    by_path = defaultdict(list)
    for u in units:
        by_path[u["path"]].append(u["symbol"])

    def resolve_symbol(file, symbol):
        """Bare-name resolution against manifest symbols declared in that file."""
        if symbol is None:
            return None
        if (file, symbol) in manifest_keys:
            return symbol
        syms = by_path.get(file, [])
        if symbol in syms:
            return symbol
        suffix = [s for s in syms if s.endswith("." + symbol)]
        if len(suffix) == 1:
            return suffix[0]
        return symbol  # unresolved bare name; keep as-is

    def entity_status(key):
        if key is None:
            return "unmapped"
        if key in comp_nodes:
            return "callable"
        if key in manifest_keys:
            return "type_or_noncallable"
        return "unknown"

    adjudication = []
    tool_pairs = set()
    for ed in edges:
        s, t = ed["source"], ed["target"]
        sk = (s["file"], resolve_symbol(s["file"], s.get("symbol"))) if s.get("file") else None
        tk = (t["file"], resolve_symbol(t["file"], t.get("symbol"))) if t.get("file") else None
        s_stat, t_stat = entity_status(sk), entity_status(tk)
        rec = {
            "source": {"key": list(sk) if sk else None, "status": s_stat,
                       "raw_symbol": s.get("symbol"), "raw_file": s.get("file")},
            "target": {"key": list(tk) if tk else None, "status": t_stat,
                       "raw_symbol": t.get("symbol"), "raw_file": t.get("file")},
            "resolved": ed.get("resolved", True),
            "raw": ed.get("raw"),
        }
        pair, rpair = (sk, tk), (tk, sk)
        if pair in comp_pairs:
            rec["verdict"] = "CONFIRMED"
            rec["direction"] = "correct"
            rec["compiler_evidence"] = [e["call_site"] for e in comp_pair_sites[pair]]
        elif rpair in comp_pairs:
            rec["verdict"] = "REFUTED"
            rec["direction"] = "flipped"
            rec["reason"] = f"compiler edge exists in reverse: {tk[1]} calls {sk[1]}"
        elif s_stat in ("unmapped", "unknown") or t_stat in ("unmapped", "unknown"):
            rec["verdict"] = "UNRESOLVED"
            rec["direction"] = None
            rec["reason"] = "endpoint could not be mapped to a compiler/manifest entity"
        else:
            rec["verdict"] = "REFUTED"
            rec["direction"] = "n/a"
            if t_stat == "type_or_noncallable":
                rec["reason"] = "target is a type declaration, not a callable"
                rec["failure_mode"] = "type_mention"
            elif s_stat == "type_or_noncallable":
                rec["reason"] = "source is a type declaration, not a callable"
                rec["failure_mode"] = "type_mention"
            else:
                rec["reason"] = ("both endpoints are fixture callables but the compiler "
                                 "finds no call in either direction")
                rec["failure_mode"] = "no_call_in_either_direction"
        # same-name collision annotation: target symbol == source symbol or
        # resolution picked the caller itself
        if rec["verdict"] == "REFUTED" and sk and tk and (
                sk[1] == tk[1] or (ed.get("raw") or {}).get("self_loop")):
            rec["failure_mode"] = "same_name_collision"
        adjudication.append(rec)
        if sk and tk:
            tool_pairs.add((sk, tk))

    vc = Counter(r["verdict"] for r in adjudication)
    flips = sum(1 for r in adjudication if r.get("direction") == "flipped")
    fm = Counter(r.get("failure_mode") for r in adjudication if r["verdict"] == "REFUTED")

    present = [p for p in sorted(comp_pairs) if p in tool_pairs]
    missed = [p for p in sorted(comp_pairs)
              if p not in tool_pairs and (p[1], p[0]) not in tool_pairs]

    res_split = Counter(("resolved" if r["resolved"] else "pending") for r in adjudication)

    def pair_of(rec):
        return [rec["source"]["key"], rec["target"]["key"]]

    confirmed_pairs = sorted(
        pair_of(r) for r in adjudication if r["verdict"] == "CONFIRMED")
    refuted_pairs = sorted(
        pair_of(r) for r in adjudication if r["verdict"] == "REFUTED")
    # Pending externals: target is outside fixture truth; identity is the
    # source (file, symbol) plus the raw unresolved target name.
    pending_externals = sorted(
        [r["source"]["key"], r["target"].get("raw_symbol")]
        for r in adjudication if r["verdict"] == "UNRESOLVED")

    return {
        "schema": "graph-lane-adjudication-v1",
        "edge_count": len(adjudication),
        "verdicts": dict(vc),
        "failure_modes": {k: v for k, v in fm.items() if k},
        "direction_flips": flips,
        "compiler_truth_pairs": len(comp_pairs),
        "compiler_pairs_present": len(present),
        "compiler_pairs_missed": [
            {"caller": a[1], "callee": b[1], "caller_file": a[0], "callee_file": b[0]}
            for a, b in missed],
        "resolved_pending_split": dict(res_split),
        "confirmed_pairs": confirmed_pairs,
        "refuted_pairs": refuted_pairs,
        "pending_externals": pending_externals,
        "edges": adjudication,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    fixture = args.fixture.resolve()
    binary = args.binary.resolve()
    root = args.work_root.resolve()

    manifest = json.loads((fixture / "manifest.json").read_text(encoding="utf-8"))
    truth = json.loads((fixture / "graph-truth.json").read_text(encoding="utf-8"))

    copy_fixture_sources(fixture, manifest, root)

    completed = subprocess.run(
        [str(binary), "--graph", str(root)], capture_output=True, text=True, check=False
    )
    if completed.returncode:
        raise SystemExit(f"zg --graph failed: {completed.stderr.strip()[:2000]}")

    codegraph = load_codegraph(root)
    edges = normalize_calls_edges(codegraph)
    result = adjudicate(edges, truth, manifest)

    with binary.open("rb") as binary_file:
        result["binary_sha256"] = hashlib.file_digest(binary_file, "sha256").hexdigest()
    result["relation_generation"] = codegraph.get("relation_generation")

    output = args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: v for k, v in result.items()
                      if k not in ("edges", "confirmed_pairs",
                                   "refuted_pairs", "pending_externals")}))


if __name__ == "__main__":
    main()
