# zg-graph-projection

Projection reader implementing issue #43's endorsed verdict for
jordigilh/zvec-grep — **"ours in front, theirs underneath"**: a reader that
projects an upstream-style in-engine code graph (zvec-ai/zvec-grep origin/main
@7504720, `zg --index --graph`) into our `codegraph-v2` sidecar format, so our
MCP/CLI query surface (`CallGraphIndex`: blast-radius, neighbors,
relation-path, affected, explain, Leiden clustering) can consume an
integrated-substrate graph **without migrating**.

This is a spike: a reader, not a migration.

## Usage

```sh
# 1. Build the graph with upstream's indexer (their binary, their freshness model)
cd <workspace> && zg --index --graph

# 2. Project it into a codegraph-v2 sidecar (our reader)
zg-graph-project <workspace-root> [--output PATH]
#    default output: <workspace>/.zvec-grep/codegraph-v2.json.zst
#    (the same location zg-codegraph publishes native sidecars to)

# 3. Query through OUR surface
zg --graph-query <workspace>/.zvec-grep/codegraph-v2.json.zst blast-radius <symbol>
```

The reader consumes three store parts under `<workspace>/.zvec-grep/index/`:

- `graph/graph.sqlite` — the edge table (status, provenance, candidates)
- `entities/` — zvec collection: entity ids -> symbol metadata + source ranges
- `files/` — zvec collection: file ids -> paths + content hashes/sizes

## Mapping table

| upstream store | codegraph-v2 |
|---|---|
| zvec entity id (32-hex, content-hashed) | node id `upstream:<entity_id>` |
| file-scope endpoint `f{file_id}` | node id `upstream:f{file_id}` (kind `file`) |
| `CodeMetadata.symbol_name` / `scope` / `signature` | node `name` / `qualified_name` / `signature` |
| `SymbolType::Function` + scope | node kind `method` (Go: `Receiver.Method`) |
| `SymbolType::Function` without scope | node kind `function` |
| `Class`/`Interface`/`Enum`/`Module`/`Alias`/`Value` | same-named kinds |
| entity payload `source_range` | node range (byte offsets + lines) |
| files snapshot `content_hash` / `size_bytes` | file `sha256` / `bytes` |
| edge kind `calls` / `contains` / `imports` / `implements` | same kind |
| edge kind `extends` | kind `inherits` |
| edge kind `declares` (internal binding evidence) | **dropped** (not in our 14-relation vocabulary) |
| status `resolved` + 9-value `provenance` | `resolved=true`, `resolution="upstream:<provenance>"` — provenance finally reaches a consumer surface |
| status `pending` (+ `reference_name`) | `resolved=false`, `target_name=reference_name` |
| status `pending` + `candidates` JSON | `ambiguous_candidates` -> possible-graph edges |
| status `failed` (external-module import refs) | `resolved=false`, `resolution="upstream:failed"` |
| edge `line`/`col` | edge range (bytes unavailable upstream; documented zero) |

The **definite/possible separation survives the mapping**: resolved edges feed
the definite call graph; pending edges with recorded ambiguity candidates feed
the possible graph (`CallGraphBlastRadius.possible_callers_by_depth`).
Pending edges with no candidates stay unresolved counts — upstream's honest
non-assertion is preserved, not converted into guessed edges.

## Validation (the gate)

Two frozen Go fixtures, upstream's indexer building the store, projection, then
our `CallGraphIndex` surface, adjudicated against compiler truth
(`go-graph-truth-v1`, RTA; identity = fixture-relative file + manifest symbol):

| Fixture | upstream reference (issue #43, via their query surface) | projection through our surface |
|---|---|---|
| `go-workflow-discovery-v1` (tuning, 18 truth edges) | 100% precision / 100% recall | **18/18 confirmed, 100% precision / 100% recall**, 0 direction flips |
| `go-inventory-audit-v1` (holdout, 17 truth edges) | 100% precision / 88.2% recall (15/17) | **16/16 confirmed, 100% precision / 94.1% recall**, 0 direction flips |

The two holdout numbers measure different things. The #43 reference (15/17)
was measured through upstream's query surface: their graph DB stores endpoints
as opaque entity IDs, so edges were reconstructed via `zg --callers`/
`--callees` and only resolved edges are exportable that way — one truth edge
was lost to the export path, not to the graph. The projection reads the
SQLite directly and recovers the graph's actual recall: 16/17. The single
holdout miss is `CheckAccess -> AssetState.Contains` — upstream's own honest
miss (receiver typed by a context-helper return; stored `pending` with no
guessed target). The projection preserves their 100% precision and the
graph's recall exactly.

### Complementarity union (holdout)

Our native resolver and upstream's disagree on *different* edges:

- ours (native) misses `HandleSelectAsset -> CheckAccess` (context-helper
  return type)
- theirs (as projected) misses `CheckAccess -> AssetState.Contains`
  (inter-procedural receiver)

Each resolver finds the edge the other misses, so the union of the two
resolved edge sets covers **17/17 truth edges, 100% precision** (report-level
union; see the PR body for the artifact-level computation).

## Not covered (follow-up work)

- **9-language extraction breadth**: the reader projects whatever upstream's
  indexer extracted (C, C++, Go, Java, JavaScript, PHP, Python, Rust,
  TypeScript). Only Go is fixture-validated here; the frozen four-language
  fixtures contain no source files to index.
- **Freshness integration**: the reader projects a store someone else built.
  Regenerating the sidecar when the upstream index changes (watching the
  generation directory, or hooking their indexing pass) is follow-up work.
- **`declares` edges**: dropped rather than mapped; they are resolver-internal
  binding evidence (variable/parameter type bindings), not relations.
- **zvec version coupling**: the reader uses the workspace's `zvec-rust` line
  (cargo's `links` rule allows only one native copy per build graph). The
  spike's fixtures were written by upstream's zvec-rust 0.8.1 indexer and read
  successfully by 0.7.2; a format change in a future upstream zvec release
  would need a reader update.
- **Edge byte offsets**: upstream records line/column only for edge sites;
  projected edge ranges carry the line/column with zero byte offsets.

## Crate layout

- `src/lib.rs` — store readers (SQLite + two zvec collections), the mapping,
  and `project_upstream_graph` / `project_upstream_index`
- `src/main.rs` — `zg-graph-project` binary
- `tests/mapping.rs` — status/kind/provenance mapping unit tests
- `tests/projection.rs` — end-to-end test over a synthetic upstream store
  (zvec collections + SQLite graph seeded exactly like upstream's writer),
  queried through `CallGraphIndex`
