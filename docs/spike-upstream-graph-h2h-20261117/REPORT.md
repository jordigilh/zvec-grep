# Upstream in-engine code graph vs zg-codegraph: head-to-head (issue #43)

Date: 2026-11-17. Upstream `zvec-ai/zvec-grep` @ `7504720` (origin/main, v0.3.1),
built release from a detached worktree (`/private/var/folders/.../T/opencode/zvec-upstream-headto-head`),
command-scoped macOS SDK flags per `docs/rust-zg-sense-baseline-results-20260926.md`.
Build: clean, 3m26s. No shared checkout, frozen fixture, or upstream branch was touched.

## 1. Adjudication vs compiler truth (go-graph-truth-v1)

Method identical to `graph-precision-shootout-output/adjudicate_shootout.py`
(endpoint identity = fixture-relative file + manifest symbol; bare-name
per-file resolution; verdicts CONFIRMED / REFUTED / UNRESOLVED; direction flips).

**Export path / limitation.** Upstream's graph DB
(`.zvec-grep/index/graph/graph.sqlite`) stores endpoints as opaque zvec entity
IDs (32-hex idmap keys), not names or paths, so the edge set is **not directly
exportable from SQLite**. Edge sets were reconstructed by running
`zg --callers` / `--callees` over every manifest symbol and parsing the CLI
text (`export_upstream.py`), exactly the fallback issue #43 anticipated. Two
consequences: (a) only **resolved** edges are recoverable — the CLI, daemon and
MCP surfaces all exclude unresolved references by design ("Unresolved
references are excluded, so empty results do not prove that a relationship is
absent"); (b) per-edge provenance is invisible to consumers (it exists only in
the `edges.provenance` column). Raw SQLite rows were inspected separately.

| Metric (calls edges) | upstream zg 0.3.1 graph | zg-codegraph + resolver-fix | naive name scan | rgctl 0.4.27 |
|---|---|---|---|---|
| Go tuning (`go-workflow-discovery-v1`, 18 truth edges) | **18 exported, 18 CONFIRMED → 100% precision, 100% recall** | 94.4% precision (resolved), 94.4% recall (missed `ListWorkflows → WorkflowCatalog.ListWorkflows`) | 26.2% | 94.4% / 94.4% |
| Go holdout (`go-inventory-audit-v1`, 17 truth edges, first measurement) | **16 exported, 16 CONFIRMED → 100% precision, 15/17 = 88.2% recall** | 100% precision, 16/17 ≈ 94.1% recall | — | — |
| Direction flips | 0 / 0 | 0 | — | 0 |
| False edges (REFUTED) | **0 / 0** | 1 (tuning, same-name self-loop) | many | 1 |
| Receiver-clash case | **resolved correctly on both fixtures** (`tools.ListAssets → AssetRegistry.ListAssets` via `file_scoped` receiver inference; same for `ListWorkflows`) | missed on tuning | wrong | missed |
| Type-mention traps | 0 | 0 | many | 0 |

Artifacts: `upstream-edges-tuning.json`, `upstream-edges-holdout.json`,
`upstream-adjudication-tuning.json`, `upstream-adjudication-holdout.json`.

**The one missed holdout edge** — `CheckAccess → AssetState.Contains`
(audit/review.go:15) — is a **non-assertion, not a false edge**: the receiver
`state` gets its type from `inventory.AssetStateFromContext(ctx)`, a
context-helper return type that tree-sitter cannot see. The reference sits in
SQLite as `status='pending'`, `receiver_name='state'`, no guessed target.
Our codegraph resolved it (its resolver-fix walks context-helper return
shapes); upstream does not, but never lies about it.

**Four-language set**: the frozen-pool fixture directories
(`python/rust/typescript-workflow-discovery-v1`) contain **no source files**
(only `replays/`); they could not be indexed by anyone. Upstream's extraction
layer nominally covers C, C++, Go, Java, JavaScript, PHP, Python, Rust,
TypeScript (9 languages vs our Go-first producer). Not measured — no fixtures.

**Incremental update check** (scratch copy, not a frozen fixture): appending a
function that calls `RecordScanned` and re-indexing added the edge (1 file
modified, 6 entities, ~2s); editing that call away removed the stale edge on
the next pass (resolved calls 17 → 16, the removed reference went pending).
Bounded invalidation + re-resolution works as documented.

## 2. Certainty-class findings

- Upstream **has** an internal certainty model, richer than expected:
  `edges.status ∈ {pending, resolved, failed}`, a 9-value `provenance`
  (`arity_bound, export_bound, file_local, file_scoped, import_bound,
  import_scoped, qualified_name_match, workspace_unique`), an ambiguity
  `candidates` JSON column, and pending references carrying
  `reference_name`/`receiver_name`/`arity`/`name_tail` evidence.
- It **never leaves the engine**. CLI, daemon and MCP expose only resolved
  edges with a one-line caveat; pending evidence, ambiguity candidates and
  provenance are invisible to consumers. There is no
  resolved-vs-pending-possible distinction in any query response, no
  per-edge confidence, and no exportable "possible callers" answer.
- `candidates` was **never populated** on either fixture (0 rows) — ambiguity
  recording (`record_candidates`) exists in the resolver but did not trigger
  here.
- Net: upstream's certainty classes are a storage-layer mechanism for
  internal correctness (honest non-assertion), not a consumer contract.
  Ours (resolved/pending certainty classes in the callfacts sidecar, published
  with fingerprints) is a *query-surface* contract. These are complementary,
  not competing, designs.

## 3. Substrate comparison

| Quality | upstream in-engine graph | our sidecar + publication manifest |
|---|---|---|
| Integration | One binary, one index dir, `--callers` next to search; IndexStore coordinates graph and zvec writes under one workspace lock | Two-step producer (zg-codegraph) + consumer; sidecar files in `.zvec-grep/` |
| Freshness | **One model**: file revisions drive both index and graph; endpoint invalidation bounded by entity IDs; stale-name/path/hierarchy evidence persisted (`zg_stale_names`) and requeued even after a crashed pass; resolution sweep runs inside every indexing pass | Sidecar freshness via publication manifest + source/context fingerprints; producer re-run is a separate step that can drift from the index |
| Transactionality | SQLite STRICT schema with CHECK constraints, `application_id` ownership proof, transactional per-file row replacement + inbound-edge invalidation; crash-writer (d94e897) leaves evidence for the next open | File-based sidecars, atomic-enough but no cross-store transaction with the vector index |
| Entity identity | Opaque content-hashed entity IDs — stable across name moves, but **unusable outside the store** and the reason full edge export requires per-symbol CLI queries | Human-readable (file, symbol) identity; sidecar is self-describing and tool-agnostic |
| Vocabulary | 6 kinds (contains, calls, imports, extends, implements, declares) | 14 relations (calls, implements, references, type-mentions, overrides, …) |
| Certainty at the surface | none (resolved-only) | resolved/pending classes, published |
| Provenance at the surface | none | compiler-attested sidecars, source/context fingerprints, publication manifest |
| Languages | 9 (tree-sitter based, per-language resolver modules incl. C++ preprocessor replay) | Go-first |
| Query surface | per-symbol callers/callees only, limit 50, no bulk export, no relation filter at CLI level (kinds filter exists in engine API) | full edge-set export (JSON sidecar), blast-radius, cluster, shortest-path |

**What we would gain on their substrate:** one freshness model tightly coupled
to indexing, SQLite transactionality and crash-safe evidence, multi-language
extraction, single-binary distribution, and the relationship queries living
next to search in every existing integration (CLI/daemon/MCP).

**What is missing there:** certainty classes and ambiguity candidates in
consumer responses, provenance/fingerprint attestation, a bulk-exportable
human-readable edge set, the wider relation vocabulary, and the
producer/consumer decoupling that lets a compiler-attested producer (gopls,
rust-analyzer, …) override heuristics.

## 4. Port price

Pieces of our differentiator stack, priced for porting onto upstream's graph:

1. **Callfacts producer contract — ports cleanly (low).** It was designed
   producer/consumer-decoupled: JSON sidecar per workspace with source/context
   fingerprints. Two options: (a) keep the sidecar and teach consumers to read
   both (zero upstream changes, but two freshness models — the thing we'd be
   trying to fix); (b) write callfacts as rows into their SQLite via a
   producer that reuses their `write_file_graph` ownership model — needs an
   `origin`/`attestation` column and a schema-version bump; their STRICT
   CHECKs mean a deliberate migration, which they currently refuse to
   auto-migrate (`no migration is provided`).
2. **Certainty classes — small in engine, the real cost is surface.** The
   storage already has status/provenance/candidates; surfacing them means
   extending `RelationshipEdge` in the engine API, then CLI/daemon-protocol/MCP
   (`zvec_grep_callers`/`callees`) in lockstep, plus a protocol version bump.
   Estimated: a focused week including tests, but every consumer (their MCP
   clients) sees new fields — an upstream-acceptance question, not a coding one.
3. **Receiver-type disambiguation — partially already there (low).** Their
   `file_scoped` provenance already reads locally-declared receiver types and
   beat our resolver on both receiver-clash fixtures. What's missing is
   inter-procedural receiver inference (context-helper return types), which is
   exactly the one edge they missed. Porting our resolver-fix heuristics into
   their `graph_resolver` bindings module is contained work (days), but it
   lives in a 4,700-line resolver we don't own.
4. **14-relation vocabulary — mechanical but wide (medium).** `GraphEdgeKind`
   is a closed enum threaded through extraction, storage CHECK constraints,
   engine API, CLI, MCP. Extending it means touching every layer plus their
   schema CHECK — straightforward but review-heavy upstream.
5. **Publication manifest / bulk export — cheap consumer-side (low).** A
   reader that walks their SQLite + idmap via the engine API and emits our
   sidecar format is a few hundred lines and gives back human-readable bulk
   edges. This is the cheapest hybrid: **their substrate as the single
   freshness/transactionality core, our consumer contract layered on top**.

Strategic cost not in the list: porting means maintaining a fork of a
4,500-line resolver + storage stack against an active upstream (their graph
commits are recent and frequent), versus our decoupled sidecar which survives
upstream churn by design.

## 5. Verdict

**Hybrid port, consumer-first — do not migrate the substrate wholesale.**

- Upstream's graph is *better than expected*: 100% precision on both Go
  fixtures, resolves both receiver-clash cases we miss, honest pending
  non-assertions, real transactional incremental freshness. It beats
  zg-codegraph on tuning (18/18 vs 17/18) and loses on holdout recall
  (15/17 vs 16/17) by exactly the context-helper-return-type case.
- But its consumer surface (resolved-only, per-symbol, opaque IDs, no
  certainty/provenance) cannot express our differentiators without upstream
  API changes we don't control, and the measured quality gap is one edge in
  each direction — not a substrate argument.
- Recommended shape: (1) keep the callfacts contract and certainty classes as
  the published consumer contract; (2) adopt the cheap hybrid — an
  engine-API reader that projects their SQLite graph into our sidecar format
  where their freshness wins matter; (3) upstream our receiver-inference
  heuristics (context-helper return types) as a contribution to their
  resolver; (4) revisit wholesale migration only if/when upstream surfaces
  pending edges and provenance in the API.

## Optional FP16 v3 re-baseline — skipped

Skipped: the frozen-pool qeval fixture inputs are absent (the four-language
fixture directories contain no source, only `replays/`), so the "rebuild under
v3 on the frozen qeval fixtures" preconditions cannot be met from the frozen
pool; and the qeval harness/baselines live outside this worktree's scope.
Expected outcome remains noise-level per the issue; re-run when the frozen
qeval inputs are re-materialized.

## Reproduction

```sh
zg=<worktree>/../zvec-upstream-headto-head-build/release/zg
$zg --index <fixture-copy> --graph --mode direct
python3 export_upstream.py $zg <fixture-copy> <manifest.json> edges.json
python3 adjudicate_upstream.py upstream edges.json <truth.json> <manifest.json> out.json
sqlite3 <fixture-copy>/.zvec-grep/index/graph/graph.sqlite \
  "SELECT kind,status,count(*) FROM edges GROUP BY kind,status;"
```
