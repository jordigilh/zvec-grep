#!/usr/bin/env python3
"""Compare top-10 rankings between the two sides of a context-cap A/B pair.

Each side is queried with the binary that built it. This matters: the index
manifest records the model's ``maxInputTokens``, and the engine treats a
context-cap change as a model-identity change, so querying an old-cap index
with the new-cap binary fails with "existing index uses a different embedding
model; rebuild the index". Querying each side with its own binary keeps the
comparison read-only and avoids rebuilds; query texts are short, so query
embeddings are identical under either cap.

Reports per-query top-10 overlap on path:span units (chunk granularity changes
with the cap, so span overlap is deflated mechanically) and on files. Divergent
rankings are written to per-query JSON files for manual inspection.

Usage:
  ZG_A_BIN=<old-binary> ZG_B_BIN=<new-binary> \
  compare-pair.py <a-root> <b-root> <queries.json> <out-prefix>
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

A_BIN = os.environ.get("ZG_A_BIN") or sys.exit("set ZG_A_BIN")
B_BIN = os.environ.get("ZG_B_BIN") or sys.exit("set ZG_B_BIN")
MODEL_CACHE = os.environ.get(
    "ZG_GREP_MODEL_CACHE", str(Path.home() / ".engram/zvec-grep/model")
)
OUT = Path(os.environ.get("ZG_AB_BASE") or ".")

HEADER = re.compile(
    r"^#(?P<rank>\d+)(?: \[[^\]]+\])?(?: matchedBy=\S+)? "
    r"(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?\s*$"
)


def run(binary: str, root: str, query: str) -> list[str]:
    completed = subprocess.run(
        [binary, "--mode", "direct", "--refresh", "off", "--limit", "10",
         "--preview", "full", "--model-cache", MODEL_CACHE,
         "--device", "cpu", "--hybrid", query],
        cwd=root, capture_output=True, text=True, check=False,
    )
    if completed.returncode:
        print(f"  query failed in {root}: {completed.stderr[-300:]}", file=sys.stderr)
        return []
    return [
        f"{m['path']}:{m['start']}-{m['end'] or m['start']}"
        for m in (HEADER.match(line) for line in completed.stdout.splitlines())
        if m
    ]


def main() -> None:
    a_root, b_root, queries_file, prefix = sys.argv[1:5]
    queries = json.loads(Path(queries_file).read_text())["queries"]

    def rel(hits, root):
        return [h[len(root):].lstrip("/") if h.startswith(root) else h for h in hits]

    span_overlaps, file_overlaps = [], []
    for q in queries:
        a = rel(run(A_BIN, a_root, q["query"]), a_root)
        b = rel(run(B_BIN, b_root, q["query"]), b_root)
        span = len(set(a) & set(b))
        file = len({h.split(":")[0] for h in a} & {h.split(":")[0] for h in b})
        span_overlaps.append(span)
        file_overlaps.append(file)
        print(f"{prefix} {q['id']} overlap@10={span}/10 file={file}/10 a={len(a)} b={len(b)}")
        (OUT / f"{prefix}-{q['id']}.json").write_text(
            json.dumps({"query": q["query"], "a": a, "b": b}, indent=1))
    n = len(queries)
    print(f"{prefix} MEAN overlap@10 span={sum(span_overlaps)/n:.2f} file={sum(file_overlaps)/n:.2f}")


main()
