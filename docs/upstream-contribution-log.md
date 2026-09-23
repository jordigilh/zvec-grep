# Upstream contribution log

This file records changes made in the local zvec-grep checkout that may be
proposed upstream. Keep entries append-only: preserve the original motivation,
files, verification evidence, and submission status when a change is revised or
split into upstream pull requests.

The local checkout's `origin` is the upstream repository
[`zvec-ai/zvec-grep`](https://github.com/zvec-ai/zvec-grep). This log is not an
upstream release changelog. Engram issue
[#115](https://github.com/jordigilh/engram/issues/115) tracks the contribution
plan; Engram issue [#113](https://github.com/jordigilh/engram/issues/113) tracks
the branch-overlay CodeGraph sidecar use case.

## Local checkout baseline

- Checkout: local zvec-grep worktree (`zvec-grep-fork`)
- Upstream remote: `https://github.com/zvec-ai/zvec-grep.git`
- Local branch: `spike/codegraph-sidecar`
- Base commit: `b5ea18d8f031d7ccae992221b10d5cab242e4e17`
- Entry status: local, uncommitted, not submitted upstream

## 2026-09-23 — LCL-001: Go codegraph sidecar and container build path

**Status:** Local work in progress; not submitted upstream.

**Purpose:** Add a deterministic Go structural index beside zvec-grep's semantic
workspace index. This is intended for disposable branch/worktree overlays where
call-graph queries should not require a separate persistent indexing service.

**Changes currently present in the worktree:**

- New `zg-codegraph` crate extracts Go files, definitions, imports, and lexical
  call edges into a versioned `codegraph-v1.json` artifact with content hashes,
  stable logical IDs, source ranges, and unresolved/ambiguous edge metadata.
- Incremental updates accept changed and deleted paths, reparse changed files,
  remove tombstoned nodes, and re-resolve call edges against the updated
  definition set.
- `petgraph` backs call-graph operations; `leiden-rs` supplies deterministic
  modularity clustering.
- `zg-engine` exposes the sidecar API, and the CLI adds `zg graph` and
  `zg graph-query` for full/delta builds, blast radius, shortest path, per-node
  cluster, and community queries.
- A Trixie/Rust 1.98 Docker build and `rust/scripts/container.sh` provide a
  repeatable container path for the native Rust build and graph commands.

**Files in the current local delta:**

- Modified: `rust/Cargo.lock`, `rust/crates/zg-cli/src/lib.rs`,
  `rust/crates/zg-cli/src/render.rs`, `rust/crates/zg-engine/Cargo.toml`,
  `rust/crates/zg-engine/src/lib.rs`, `rust/crates/zg/src/main.rs`.
- Added: `rust/.dockerignore`, `rust/Dockerfile`,
  `rust/crates/zg-codegraph/Cargo.toml`,
  `rust/crates/zg-codegraph/src/lib.rs`,
  `rust/crates/zg-codegraph/src/graph_queries.rs`,
  `rust/scripts/container.sh`.

**Verification previously reported during the local evaluation:** 43 CLI tests,
13 codegraph tests, strict Clippy, a Trixie release build, and graph CLI smoke
queries passed. Re-run the Rust contributor guide's complete gate before any
upstream submission; this log entry itself does not constitute a fresh test run.

**Upstream disposition:** Not yet split into reviewable commits; no upstream
issue or pull request opened. Reassess the public API, artifact lifecycle,
incremental update contract, and maintainers' preferred scope before proposing
the sidecar upstream.

## 2026-09-23 — LCL-002: CocoIndex comparison findings (candidate, not implemented)

**Status:** Evidence captured; no semantic-search source changes made.

The comparison used Kubernaut branch
`fix/2442-workflow-discovery-membership`, HEAD
`d7df5737ae9d14413ef68466324639f525574fee`, base
`70b9d854cc21b83e8740909d69b2f6aa63d3d6c4`, and dirty-worktree digest
`359a9c40c6e25da074cc3eafeace24d7a6e00af88b9d7153dae80bb49d04c915`. The
matched corpus was 1,071 production Go files / 13,581,851 bytes. Exact prompts
are recorded in Engram's `benchmarks/semantic_search/kubernaut_workflow_discovery.json`.

### Finding A — hybrid fusion can demote a strong lexical result

For “How does workflow discovery membership persist across self-correction and
retry attempts?”:

- FTS-only ranked `Validator.SetDiscoveredWorkflowState` first.
- Vector-only ranked unrelated retry functions first.
- Hybrid retrieved the exact target at FTS rank 1 / vector rank 84, but fused it
  to rank 9 (score `0.0233378874`).
- An unrelated CRD retry hit at FTS rank 10 / vector rank 2 and won fused rank 1
  (score `0.0304147465`).

The implementation in
`rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs::fuse_candidates`
adds equal-weight reciprocal-rank contributions `1 / (60 + rank)`. This is
direct evidence for a fusion regression case. It does not decide whether the
fix should use route weights, adaptive fusion, or a reranker.

For “What happens when the model selects a workflow that exists in the catalog
but was not returned by workflow discovery?”, `Validator.IsAllowed` was FTS
rank 2 / vector rank 36 / fused rank 5, while the less-direct `buildFinalResult`
candidate (FTS rank 3 / vector rank 11) won fused rank 1.

### Finding B — code documentation is indexed but omitted from the CLI preview

`lexical_text` in `rust/crates/zg-engine/src/pipelines/indexing/pipeline.rs` and
`vector_metadata_text` in `rust/crates/zg-engine/src/extraction/service.rs`
include `CodeMetadata.documentation` in indexed search text. However,
`write_item_preview` in `rust/crates/zg-cli/src/render.rs` renders code symbol
and scope metadata without rendering its documentation. A relevant code hit can
therefore omit the comment that explains why it answers a natural-language
query. Candidate change: expose documentation in the result contract/preview
with a tested size policy.

### Finding C — file discovery and snippet relevance differ

For “Only allow the model to select remediation workflows discovered in this
investigation context”, CocoIndex found `validator.go` at unique-file rank 2;
zvec found it at unique-file rank 8. zvec's exact `IsAllowed` entity was FTS
rank 76 / vector rank 39 / hybrid rank 10. The reported CocoIndex rank-2 chunk
was catalog-metadata enrichment rather than the guard itself, and the saved
comparison did not retain its complete CocoIndex result list. Keep this as a
file-recall lead; do not claim a relevant-snippet win without full chunk
judgments.

**Next evidence required:** retain full ranked chunks, replay the fixed query
suite, report file-recall separately from chunk-level relevance, and preserve
per-route ranks plus fused ranks. Add regression tests before choosing a fusion
implementation. No fix from LCL-002 has been applied yet.

## Entry template

For each future change, append an entry containing:

- Date and stable local entry ID.
- Source issue and motivation/evidence.
- Local base/upstream revision and local commit(s) or uncommitted paths.
- Behavior and files changed.
- Tests/commands and actual results.
- Upstream disposition: `local`, `validated`, `submitted`, or `merged`, with
  issue/PR links when applicable.

## 2026-09-23 — LCL-003: Upstream-base multi-language codegraph and MCP tools

**Status:** Local work in progress; not submitted upstream.

**Base and branch:** Reconciled the preserved POC onto local-only branch
`integration/zvec-live-code-intelligence`, based on upstream `main` at
`f6358f4`. The prior checkout state remains recoverable as `stash@{0}`. The
integration branch has no push upstream configured.

**Changes currently present in the worktree:**

- Retained upstream's default-search CLI and added graph actions as
  `zg --graph` / `zg --graph-query`, with planning and help text.
- Generalized the graph sidecar to Go, Rust, TypeScript, TSX, and Python while
  retaining Go-only compatibility wrappers. Artifacts include a language field
  with a Go default when reading earlier v1 sidecars.
- Added persisted, hash-based graph refresh that reparses only added or changed
  supported files and removes deleted files before re-resolving calls.
- Added four root-required MCP graph tools (blast radius, shortest path, cluster,
  communities) to the default `agent` toolset without exposing index/delete
  administration. Graph indexes are cached by canonical
  workspace root and refreshed before querying.
- Added daemon-level MCP coverage proving an edited Python source refreshes the
  root-scoped callgraph and removes a stale caller edge.
- Kept the bounded code-documentation preview candidate and added short/full
  rendering coverage.
- Started a local integration backlog in `docs/local-integration-issues.md`;
  no user-owned zvec-grep fork was found, so no external issues or pull requests
  were created.

**Verification:**

- `cargo fmt --all` — passed.
- `cargo test -p zg-codegraph` — 16 passed.
- `CXXFLAGS='-isysroot /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk -isystem /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/c++/v1' cargo test -p zg-cli` — 52 passed.
- The same `CXXFLAGS` prefix with `cargo test -p zg-transport-mcp` — 33 passed.
- The same `CXXFLAGS` prefix with
  `cargo test -p zg --test server_lifecycle -- --test-threads=1` — 13 passed.
- The same `CXXFLAGS` prefix with `cargo clippy -p zg-codegraph -p zg-cli
  -p zg-transport-mcp -p zg --all-targets -- -D warnings` — passed.
- The explicit C++ include flags are required on this host because the default
  Command Line Tools compiler invocation does not discover the SDK's libc++ headers.

**Files in the current local delta:** `docs/local-integration-issues.md`,
`docs/upstream-contribution-log.md`, `rust/crates/zg-codegraph/`,
`rust/crates/zg-cli/src/lib.rs`, `rust/crates/zg-cli/src/render.rs`,
`rust/crates/zg-transport-mcp/Cargo.toml`,
`rust/crates/zg-transport-mcp/src/lib.rs`,
`rust/crates/zg/src/main.rs`, `rust/crates/zg/tests/server_lifecycle.rs`, plus
the preserved POC changes listed in LCL-001 and LCL-002.

**Upstream disposition:** Local only. Review the artifact/API compatibility,
parser coverage, service performance, and public MCP toolset placement before
splitting this into upstream proposals.

## 2026-09-23 — LCL-004: Route-aware fusion and code-documentation previews

**Status:** Implemented locally and replayed against the fixed exploratory
suite; not submitted upstream. LCL-002's “no semantic-search source changes”
describes the state when that finding was recorded.

**Evidence and behavior:**

- Hybrid scoring uses RRF K=60, FTS weight 1.1, and vector weight 1.0. Routes
  receive full weighted RRF when their ranks are within 2.5x and both fall in
  the result window or both are deeper than twice the requested limit. Otherwise
  the strongest route leads and the other contributes 7%; this protects strong
  route hits when evidence disagrees or straddles the transition band.
- Short code previews expose up to 320 characters of documentation; full
  previews include all indexed code documentation and `none` omits it.
- Replayed all seven active prompts from
  `benchmarks/semantic_search/kubernaut_workflow_discovery.json` at limit 10
  against the preserved 1,071-file Kubernaut index, without reindexing. The
  exact `Validator.SetDiscoveredWorkflowState` hit moved from historical hybrid
  rank 9 to rank 4 (FTS 1 / vector 84; fused score `0.018518898`). The exact
  `Validator.IsAllowed` hit for the file-recall lead moved from historical
  hybrid rank 10 to rank 3 (FTS 76 / vector 39; fused score `0.018189245`).
  Other first relevant symbols in the exploratory replay included
  `Validator.IsAllowed` at rank 1 for the catalog-valid rejection query,
  `DiscoveredWorkflowState.Add` at rank 2 for recording listed workflow IDs,
  and `filtersFromSignal` at rank 2 for label-driven filters. These prompts have
  no adjudicated relevance labels; they are observed ranks, not a parity claim.

**Files changed for this entry:**

- `rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs` — route
  scoring and regression tests.
- `src/engine/pipeline/search/fusion.ts` and
  `src/engine/pipeline/search/index.ts` — matching TypeScript implementation.
- `rust/crates/zg-cli/src/render.rs` and `src/cli/format/context.ts` — bounded
  code-documentation previews and tests in the corresponding test files.
- `docs/02-cli.md`, `docs/04-pipeline.md`, and `docs/05-architecture.md` —
  preview and fusion behavior.

**Verification:**

- TypeScript: `npm run lint`, `npm run format:check`, `npm run typecheck`,
  `npm run build`, and `npm run test:run:unit` passed (279 passed, 1 skipped).
- Rust 1.98.0 Trixie container: `cargo fmt --all -- --check`; `cargo test
  --locked -p zg-engine --lib` (436 passed, 7 ignored); `cargo test --locked -p
  zg-cli --lib` (52 passed); `cargo clippy --locked --no-deps -p zg-engine
  --all-targets -- -D warnings`; and `cargo build --locked --release -p zg` all
  passed.
- Strict Clippy over the wider uncommitted CodeGraph/CLI work still reports
  existing lints outside this entry; it was not treated as a pass.
- The public `local/potion-code-16m-v2` model assets were downloaded once and
  persisted at `/Users/jgil/.engram/zvec-grep/model` (snapshot
  `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`). The replay reused the preserved
  index and this cache.

**Upstream disposition:** Local only. The seven-query replay supports the
specific regression cases but does not establish general quality or CocoIndex
parity. Keep LCL-002's file-retrieval/snippet-relevance distinction; the full
CocoIndex chunk list is still unavailable.

## 2026-09-23 — LCL-005: Main-aligned retrieval comparison and capped corroboration

**Status:** Local experiment; not submitted upstream. Source issue: Engram
[#115](https://github.com/jordigilh/engram/issues/115).

**Corpus alignment:**

- Verified remote `origin/main` at
  `70b9d854cc21b83e8740909d69b2f6aa63d3d6c4`; created a detached main worktree
  and indexed 1,067 production Go files / 13,488,831 bytes in zvec.
- Built isolated CocoIndex shadow table
  `cocoindex.code_embeddings_shadow_kubernaut_main_70b9d854_canonical` from the
  same file set. It contains 19,197 chunks across 1,067 distinct files. The live
  `cocoindex.code_embeddings` table was not changed.
- Models differ (`local/potion-code-16m-v2` vs.
  `sentence-transformers/all-MiniLM-L6-v2`); raw scores are not comparable.
  No BM25-ranked item appeared in the returned CocoIndex top 10 for any of the
  seven base prompts.
- Important branch caveat: the active launchd configuration points `ENGRAM_CODE_DIR`
  at the live Kubernaut clone and its comment says ingestion follows the current
  checkout. That checkout was on `fix/2442-workflow-discovery-membership` during
  this run. `search_code(branch="main")` excludes release-tagged rows; it does
  not pin a Git commit. Therefore only the isolated table above is used as the
  verified main-to-main comparison.

**Triage and implementation:**

- Added a 25% cap on cross-route corroboration when route ranks are considered
  close, retaining the 7% contribution for discordant/transition-band evidence.
  A main-snapshot regression case showed the prior deep-rank agreement (FTS 30 /
  vector 25) outranking a stronger lexical hit (FTS 2 / vector 47); after the
  cap, `runWorkflowSelection` moved from zvec rank 7 to rank 1. CocoIndex still
  returns the more exact `Validator.IsAllowed` chunk at rank 1; that exact entity
  remains absent from zvec's top 10 for this prompt.
- Other observed differences: zvec returns `filtersFromSignal` at rank 1 for
  label-to-filter flow; CocoIndex leads with a `Workflow` model chunk. For the
  explicit result-guard follow-up, CocoIndex returns
  `isWorkflowInDiscoveryResult` first, while zvec returns `SelectWorkflowTool.Handle`
  first and `buildFinalResult` second. These are inspection leads, not graded
  wins; prompts remain unlabelled.
- Added three agent-generated follow-up probes in Engram's
  `benchmarks/semantic_search/kubernaut_workflow_discovery_followups_v1.json`.
  They are kept separate from actual captured user/CocoIndex search logs.
- The path/chunk/symbol and route-rank snapshot is recorded at Engram's
  `benchmarks/semantic_search/kubernaut_main_70b9d854_2026-09-23.json`.

**Code and verification:**

- Updated `rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs`,
  `src/engine/pipeline/search/fusion.ts`, regression tests, and fusion docs.
- TypeScript lint, Prettier, typecheck, build, and unit tests passed (279 passed,
  1 skipped). Rust formatting, engine tests (436 passed, 7 ignored),
  engine-only strict Clippy, and `cargo build --locked --release -p zg` passed.
- The live CocoIndex search table and current Kubernaut source checkout were not
  modified. The isolated shadow table and detached main worktree are retained
  for further iterations.

**Upstream disposition:** Local and experimental. The fixed query suite has no
adjudicated snippet labels; do not claim parity or general quality improvement.

## 2026-09-23 — LCL-006: Main-aligned Kubernaut result triage

**Status:** Supplemental main-to-main comparison record; not submitted upstream.

**Corpus alignment:**

- Verified remote `origin/main` at
  `70b9d854cc21b83e8740909d69b2f6aa63d3d6c4`; used a detached main worktree.
- Both backends searched the same 1,067 production Go files / 13,488,831 bytes
  (exclude vendor, `*_test.go`, `zz_generated*`, and files over 2 MiB).
- zvec indexed 17,484 entities with `local/potion-code-16m-v2`. CocoIndex's
  isolated main shadow contains 19,197 chunks from the same 1,067 files, using
  `sentence-transformers/all-MiniLM-L6-v2` with 1,000-character chunks and
  300-character overlap. Raw model scores are not compared.
- The live `cocoindex.code_embeddings` table was not changed. The deployed
  launchd plist points at the live Kubernaut checkout, which follows its current
  Git checkout; `search_code(branch="main")` excludes release-tagged rows but
  does not pin a commit. The branch-pinned comparison therefore uses isolated
  table `cocoindex.code_embeddings_shadow_kubernaut_main_70b9d854_canonical`.

**Observed differences and triage:**

- On “reject a catalog-valid workflow never discovered by `list_workflows`”,
  CocoIndex ranks `Validator.IsAllowed` first. zvec's closest validation hook
  (`runWorkflowSelection`) was rank 7 before the corroboration cap; after the
  cap it is rank 1 (FTS 2 / vector 47). The exact `IsAllowed` entity is still
  absent from zvec's top 10, so this is a partial improvement, not a fix.
- A result-guard follow-up ranks CocoIndex's
  `isWorkflowInDiscoveryResult` first; zvec returns `SelectWorkflowTool.Handle`
  first and `buildFinalResult` second. Triage candidate: inspect chunk/entity
  granularity and whether the validator guard's indexed text is being recalled.
- For label-to-filter flow, zvec returns `filtersFromSignal` first,
  `WorkflowSearchFilters` second, and `applyDiscoveryFilterData` third. CocoIndex
  leads with a `Workflow` model chunk; this is a zvec snippet-level strength to
  preserve.
- For the seven base prompts, unique-file overlap within the returned top 10
  ranges from 0 to 4 files. This is a diagnostic only, not a quality score. No
  BM25-ranked item appears in the returned CocoIndex top 10 for any base prompt;
  inspect this before interpreting these results as hybrid retrieval.
- Three agent-generated follow-up probes and ranked result summaries are
  recorded in Engram's `benchmarks/semantic_search/kubernaut_main_70b9d854_2026-09-23.json`
  and `kubernaut_workflow_discovery_followups_v1.json`. They are explicitly
  separate from actual captured CocoIndex user-query logs.

**Local scoring iteration:**

- Route-agreement corroboration is capped at 25% of the strongest weighted
  route score. This specifically prevents deep-rank agreement from outranking a
  much stronger near-top result. Discordant or transition-band evidence still
  uses the 7% secondary contribution.
- Added a regression case for the observed FTS rank 2 / vector rank 47 result
  versus a deep FTS rank 30 / vector rank 25 agreement.

**Verification:** TypeScript lint, formatting, typecheck, build, and unit suite
passed (279 passed, 1 skipped). Rust formatting, `zg-engine` tests (436 passed,
7 ignored), engine-only strict Clippy, and the release build passed. The complete
comparison artifact records final post-cap rankings.

**Upstream disposition:** Local, uncommitted, and not submitted. The prompts
remain unlabelled; the main-to-main output is for inspection and triage only.

## 2026-09-23 — LCL-007: Identifier-anchored candidate-recall probe

**Status:** Exploratory follow-up; no generic query rewrite implemented.

For the natural-language “reject a catalog-valid workflow” prompt, the exact
`Validator.IsAllowed` entity ranks FTS 163 / vector 110 in zvec's limit-200
route diagnostics. A new probe, “`Validator.IsAllowed discoveredWorkflows
list_workflows catalog allowlist`”, moves the same entity to FTS 1 / vector 1
and hybrid rank 1. CocoIndex also returns the validator chunk first for that
anchored prompt.

Fusing the original hybrid query with the identifier-anchored FTS query using
`zg --fuse --hybrid <natural query> --fts <anchored query>` returns
`workflowCatalogFetcher.FetchValidator` at rank 1 and `Validator.IsAllowed` at
rank 2. This suggests the remaining gap is candidate recall/query vocabulary,
not only final fusion. Treat automatic identifier expansion as a follow-up
candidate and test it on additional codebases and prompts before implementation.

The exact probe and route traces are recorded in Engram's main comparison
artifact and `kubernaut_workflow_discovery_followups_v1.json`.

**Upstream disposition:** Local evidence only; not submitted.

## 2026-09-23 — LCL-008: Current fix/2442 branch comparison

**Status:** Local branch-focused replay; no source changes in this entry.

**Corpus alignment:**

- Kubernaut branch `fix/2442-workflow-discovery-membership`, HEAD
  `8f3bc5a2d7da5262553a0919f9faecb83d61a09a`; selected-Go snapshot digest
  `1eb2a57192e0af48c80741d7d732a13028f93c2614d194e3ce278234cc859dd8`.
- Materialized an isolated zvec root containing 1,072 matching Go files / 13,611,586
  bytes and indexed 17,629 entities. The live CocoIndex table has 19,371 rows
  across the same 1,072 distinct Kubernaut paths. The active table was queried
  read-only; this replay did not rebuild or modify it.
- CocoIndex returned no BM25-ranked result in the top 10 for the seven base
  prompts or four follow-ups. Model scores are not comparable.
- The separate `origin/main` run in LCL-006 remains historical context only;
  this entry uses only the current fix/2442 branch results.

**Triage:**

- For the natural catalog-valid rejection prompt, zvec returns
  `Validator.IsAllowed` at rank 1 (FTS 4 / vector 6) and
  `runWorkflowSelection` at rank 2; CocoIndex's first chunk is also in
  `parser/validator.go`. This is materially better aligned than the main run.
- For workflow-ID recording, zvec returns `FetchValidator` rank 1 and
  `DiscoveredWorkflowState.Add` rank 2; `SetDiscoveredWorkflowState` is rank 4.
- For retry persistence, zvec returns `SetDiscoveredWorkflowState` rank 1
  (FTS 1 / vector 84); CocoIndex surfaces the same file/chunk in its follow-up
  list. For selection-result guarding, CocoIndex ranks
  `isWorkflowInDiscoveryResult` first and zvec ranks `SelectWorkflowTool.Handle`
  first, with `buildFinalResult` second.
- The unrestricted “only allow” query still has zero unique-file overlap in the
  top 10 and is a high-priority gap. Limit-100 route diagnostics show relevant
  vector candidates `GetWorkflowWithContextFilters` (rank 5) and
  `convertWorkflowsToDiscoveryEntries` (rank 12); the final fused list puts them
  at 14 and 25, while `Validator.IsAllowed` (FTS 76 / vector 39) lands at 40.
  This is a ranking/fusion gap after candidate retrieval, not a missing-from-recall
  case. The label-to-filter query continues to surface direct functions
  (`applyDiscoveryFilterData`, `filtersFromSignal`) near the top in zvec.
- For the state-flow prompt, vector-only ranks
  `DiscoveredWorkflowStateFromContext`, `WithDiscoveredWorkflowState`, and
  `SetDiscoveredWorkflowState` at 1, 2, and 3; hybrid final ranks are 9, 10, and
  7. This points to a query-group/intent coverage issue that a symbol result or
  secondary query group may address.
- Ran an equal-budget query-group spike: the single hybrid top 10 includes
  `IsAllowed` at 1 but the state/context API symbols only at 7, 9, and 10. Adding
  a state-focused vector group (limit 5 per group, 10 total results) places
  `DiscoveredWorkflowStateFromContext`, `WithDiscoveredWorkflowState`,
  `NewDiscoveredWorkflowState`, and `SetDiscoveredWorkflowState` at 2, 4, 6,
  and 8. For the unrestricted-selection prompt, keeping an identifier-anchored
  FTS query as a separate group returns `IsAllowed` at 2, `SetDiscoveredWorkflowState`
  at 4, and `Validator.Validate` at 6; `--fuse` moves `IsAllowed` to 3. This
  supports testing group coverage before another global fusion-weight change.
- Exact ranked paths, chunks/entities, route ranks, and overlap counts are
  recorded at Engram's
  `benchmarks/semantic_search/kubernaut_fix-2442_8f3bc5a2_2026-09-23.json`.

**Upstream disposition:** Local evidence only; no parity claim and no commit.

## 2026-09-23 — LCL-010: Generic agent-side query-group planning guidance

**Status:** Shared local guidance spike; not submitted upstream. It requires no
JEV/ModernBERT dependency and adds no project-specific query templates.

**Behavior:** For a query that explicitly asks about multiple stages, causes, or
handoffs, preserve the user's original wording as the primary hybrid group and
add at most one supplemental lexical or vector group for a distinct facet. Keep
groups separate (`fuse` omitted) and use a small per-group limit. Atomic queries,
exact lookups, and uncertain decompositions keep the existing single-query path.
This moves query decomposition into the already-running agent model while
bounding extra retrieval calls/results.

**Evidence:** The fix/2442 spike used the same total 10-result budget. For the
state-flow query, one hybrid group returned `IsAllowed` first and state/context
methods at 7, 9, and 10. Adding one focused vector group moved
`DiscoveredWorkflowStateFromContext`, `WithDiscoveredWorkflowState`,
`NewDiscoveredWorkflowState`, and `SetDiscoveredWorkflowState` to ranks 2, 4, 6,
and 8. For “only allow”, a supplemental identifier-anchored FTS group placed
`IsAllowed` at rank 2; fusing it into the same plan placed it at rank 3.

**Files:** `src/cli/install.ts` adds the shared managed agent guidance;
`docs/01-agents.md` and `docs/03-mcp.md` document the generic policy and API
shape; `test/install.test.mjs` verifies it survives installation to Codex and
OpenCode guidance.

**Verification:** `npm run lint`, `npm run format:check`, `npm run typecheck`, and
`npm run build` passed. `node --test test/install.test.mjs` passed (74 tests).

**Upstream disposition:** Local guidance change; broader cross-project answer
quality and classifier generalization remain unvalidated.

## 2026-09-23 — LCL-011: Issue-linked integration commits and fork tracking

**Status:** Local integration changes are recorded on
`integration/zvec-live-code-intelligence`; no upstream pull request has been
opened. `origin` remains `zvec-ai/zvec-grep` and `fork` is
`jordigilh/zvec-grep`.

**Issue-linked commits:**

- `ebd032d feat(codegraph): add multi-language graph tools` — fork issue
  [#3](https://github.com/jordigilh/zvec-grep/issues/3).
- `11358e8 fix(search): guard hybrid fusion against route disagreement` — fork
  issue [#2](https://github.com/jordigilh/zvec-grep/issues/2).
- `e6d3eae docs(agents): guide multi-stage query grouping` — fork issue
  [#4](https://github.com/jordigilh/zvec-grep/issues/4); installer build and all
  74 `test/install.test.mjs` tests passed.
- Kubernaut/CocoIndex parity evidence and remaining quality gates are tracked in
  fork issue [#1](https://github.com/jordigilh/zvec-grep/issues/1) and
  `docs/local-integration-issues.md`.

LCL-003's earlier note that no user-owned fork or external issues existed records
the pre-fork state. The dedicated branch preserves `origin` for upstream and
uses `fork` for the personal integration target. The parity issue remains open:
the seven-prompt replay is exploratory and has no adjudicated relevance labels.

**Upstream disposition:** Local issue-linked work only; upstream submission
scope and timing remain undecided.
