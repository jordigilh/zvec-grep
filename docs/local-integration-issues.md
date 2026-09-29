# Local zvec integration issue tracker

This backlog tracks the local Engram/Kubernaut integration against zvec-grep.
`origin` remains upstream; `fork` is `jordigilh/zvec-grep`. The fork issues
tracking the work are [#1 Kubernaut parity](https://github.com/jordigilh/zvec-grep/issues/1),
[#2 retrieval fusion](https://github.com/jordigilh/zvec-grep/issues/2),
[#3 graph/MCP](https://github.com/jordigilh/zvec-grep/issues/3), and
[#4 query-group guidance](https://github.com/jordigilh/zvec-grep/issues/4). Engram
issues [#115](https://github.com/jordigilh/engram/issues/115) and
[#113](https://github.com/jordigilh/engram/issues/113) remain the cross-repo
contribution and branch-overlay references.

## ZGI-001 — Rebase local POC on current upstream

**Status:** Issue-linked changes are committed on the dedicated integration branch;
upstream `main` remains untouched.

- Created `integration/zvec-live-code-intelligence` from upstream `main` at
  `f6358f4`, with `origin` retained for upstream and `fork` set to the personal
  fork.
- Preserved the previous uncommitted POC in `stash@{0}` and restored its changes
  onto the integration branch. The stash remains available as a recovery copy.
- Kept unrelated Engram overlay-spike changes untouched.
- Logical commits: `ebd032d` (graph/CLI/MCP), `11358e8` (hybrid fusion), and
  `e6d3eae` (query-group guidance), linked to fork issues #3, #2, and #4.

## ZGI-002 — Multi-language, branch-fresh callgraph sidecar

**Status:** Core implementation complete; upstream review scope remains open.

- Extract Go, Rust, TypeScript, TSX, and Python definitions, imports, and lexical
  call edges with tree-sitter.
- Persist the sidecar under each canonical checkout root and refresh it from
  current file hashes, including added, modified, deleted, and uncommitted files.
- Tests cover all five languages, incremental refresh, source-stamp changes, and
  deletion/rename behavior.
- Follow up on parser edge cases and compare sidecar coverage against CocoIndex
  over the fixed Kubernaut corpus.

## ZGI-003 — Root-scoped graph MCP tools

**Status:** Implemented and exposed by the running local Rust `agent` toolset.

- Added callers/blast-radius, shortest-path, cluster, and communities tools to
  the default agent toolset without exposing index/delete administration.
- Require an absolute root. Cache graph query indexes by canonical root and
  refresh the persisted graph before each query, so different worktrees do not
  share graph state.
- The daemon integration test verifies that a second MCP call observes changed
  Python source and no longer reports the old caller edge.
- Wire the tools into Kubernaut's configured zvec-grep daemon, then verify calls
  against both the current branch and a second worktree.

## ZGI-004 — zvec-primary semantic shadow comparison

**Status:** Active in the local Kubernaut Engram route; continuous shadow
logging is enabled. Quality acceptance is pending.

- Route Kubernaut semantic queries to zvec-grep as primary.
- Run CocoIndex asynchronously as a non-blocking shadow on the same live
  checkout; record arguments, branch/commit, ranked results, latency, and errors.
- Keep results and ranks from each backend separate; shadow failures must not
  affect primary responses.
- The local Engram adapter shadows semantic search and graph queries only when
  the supplied canonical root matches a configured CocoIndex live source. It
  marks CocoIndex as the comparison reference and logs full responses,
  semantic file-rank overlap/rank deltas, graph caller differences, and
  branch/commit/dirty metadata to
  `~/.engram/logs/zvec-cocoindex-shadow.jsonl`.
- The release zvec-grep daemon uses `~/.engram/zvec-grep` for runtime/model
  configuration and the Kubernaut checkout's ignored `.zvec-grep/` directory
  for branch-bound indexes and graph state.
- The live replay indexed 1,071 production Go files and compared seven semantic
  prompts plus a caller graph query on branch
  `fix/2442-workflow-discovery-membership` at HEAD
  `bb7af7f4b83ddc0a719ed6748c430ae2f09e68d6`. Only one of the seven queries had
  the same top file at rank 1; CocoIndex-only and zvec-only paths are recorded
  for each query. CocoIndex remains the evaluation reference. The queried
  two-level caller chain matched, while graph totals differ: zvec 81,034 calls,
  42,706 unresolved, 21,791 ambiguous; CocoIndex 67,292, 39,299, and 15,997.
- Keep observing `~/.engram/logs/zvec-cocoindex-shadow.jsonl`; promote zvec from
  pilot only after resolving material ranking/freshness differences and
  adjudicating representative results.

## ZGI-005 — Retrieval quality and regression gates

**Status:** Evidence captured; fixed-corpus rerun pending.

- Retain complete ranked chunks for the existing CocoIndex comparison prompts.
- Report unique-file recall separately from chunk-level relevance and retain
  per-route plus fused ranks.
- Add regression cases for the observed hybrid-fusion demotion before changing
  production ranking weights.
- The live zvec/CocoIndex replay compared seven exploratory prompts on the same
  Kubernaut worktree. The first file matched at rank 1 for one prompt; the other
  top-file lists differed. This is a discrepancy signal, not a quality score,
  because the suite has no adjudicated relevance labels.
- For the graph probe, the two-level caller chain matched exactly. Aggregate
  counts did not: zvec reported 81,034 calls / 42,706 unresolved / 21,791
  ambiguous, while CocoIndex reported 67,292 / 39,299 / 15,997. Verify graph
  file scope and parser/resolution semantics against the 1,071-file corpus.

## ZGI-006 — Focused query groups for multi-stage questions

**Status:** Implemented in local managed agent guidance; tracked in fork issue
[#4](https://github.com/jordigilh/zvec-grep/issues/4).

- Preserve the user's original wording as the primary hybrid group; add at most
  one supplemental vector or FTS group for an explicit distinct facet.
- Keep groups separate, use a small per-group budget, and do not invent symbols.
- Installer tests verify the guidance survives Codex and OpenCode configuration
  generation. Broader cross-project relevance validation remains pending.

## ZGI-007 — Four-language Graphify code-graph parity and semantic advantage

**Status:** Active local issue; design captured in
[`docs/four-language-codegraph-parity-design-20260928.md`](./four-language-codegraph-parity-design-20260928.md).
Fork PR [#24](https://github.com/jordigilh/zvec-grep/pull/24) is open as a
draft against `integration/zvec-live-code-intelligence`; no upstream PR is open
yet.

### Scope

Close the remaining Graphify **code-only** feature gaps for Go, Rust,
TypeScript/TSX, and Python. Hindsight owns documents and other non-code
objects; all other programming languages are out of scope.

### Current baseline

- Root-scoped structural graphs already contain source files, declarations,
  `defines`, `imports`, and `calls` edges.
- Blast radius, shortest path, Leiden communities, incremental refresh, CLI,
  and MCP surfaces already exist.
- Independent opt-in semantic producers exist for all four languages, with
  source/context validation and syntax fallback.
- Go cross-package semantic blast-radius behavior is validated. Rust,
  TypeScript, and Python semantic holdouts still need multi-file validation.
- The first generic relation-aware follow-up is implemented on top of the v1
  artifact: versioned relation kinds, node/neighbors/relation-path/explain
  queries, CLI/MCP surfaces, and checked-in four-language multi-file fixtures
  for syntax-attested inheritance, implementation, references, and tests.

### Parity gap to close

1. Generalize the artifact from a callgraph overlay to a four-language code
   graph IR with stable node and relation semantics.
2. Add source-anchored `inherits`, `implements`, `overrides`, `mixes_in`,
   `references`, `tests`, and package/module dependency relationships where
   the language provides defensible evidence.
3. Expand queries from function/method callers to generic code nodes,
   neighbors, relation-filtered paths, affected subgraphs, and explanation
   output with edge provenance.
4. Add independent multi-file fixtures for all four languages covering imports,
   re-exports/aliases, duplicate names, receiver methods, inheritance and
   implementation relationships, and stale-context behavior.
5. Define a consistent snapshot publication boundary for the structural graph
   and accepted semantic overlays.
6. Validate incremental update, ignore/selection, cache, and current-source
   behavior against the code-only comparator.

### Opportunities to surpass Graphify

- Keep definite, possible, ambiguous, external, and unresolved relationships
  separate instead of collapsing them into ordinary calls.
- Attach exact source ranges, producer identity, toolchain/configuration, and
  context fingerprints to semantic relationships.
- Use compiler-backed binding for these four languages while retaining a useful
  syntax-only fallback.
- Reject stale or cross-context facts rather than silently presenting stale
  code relationships.
- Integrate graph traversal with source-grounded zvec code retrieval.

### Acceptance criteria

- All four languages have independent multi-file relation truth fixtures.
- The agreed code relation vocabulary is serialized and queryable with explicit
  provenance/status.
- `blast_radius`, shortest path, neighbor, affected-subgraph, and explanation
  queries work across files and relation filters.
- Fresh and incremental builds produce the same validated snapshot for the same
  source/context inputs.
- Differential Graphify comparison and semantic advantage reports are
  reproducible, fixture-scoped, and do not add Graphify as a runtime dependency.
- Unsupported or dynamic behavior remains explicitly uncertain rather than
  being promoted to definite edges.

## ZGI-008 — Rehome Graphify parity work onto a review branch

**Status:** Resolved handoff; the Graphify parity work is now committed on the
dedicated review branch and tracked by fork PR
[#24](https://github.com/jordigilh/zvec-grep/pull/24).

### Branch provenance

The source checkout for this work was `spike/rust-zg-sense-baseline`. Its
original purpose is documented in
[`docs/rust-zg-sense-baseline-plan.md`](./rust-zg-sense-baseline-plan.md):
port the Sense-inspired qualified-name and identifier-part lexical projection
into the Rust executable. That branch already contains the Sense commits and
the earlier code-IR/integration stack.

The original Sense work is already isolated in fork PR
[#22](https://github.com/jordigilh/zvec-grep/pull/22), from
`spike/rust-zg-sense-baseline` into
`integration/zvec-live-code-intelligence`. Its remote head is `76941c9`; the
later local Graphify/codegraph commits and uncommitted parity work are not part
of that PR and must not be pushed to its head branch.

The later Graphify/codegraph work was layered onto the branch in these committed
steps:

- `f1b04cd` through `b041814`: Go call-facts certainty, context attestation,
  Graphify comparison fixtures/benchmark, and contribution proposals;
- the then-uncommitted changes: Rust, TypeScript, and Python producers,
  four-language consumer validation, synthetic holdouts, benchmark updates, and
  parity design/issue documentation.

The original codegraph effort is already identified by ZGI-002/ZGI-003, fork
issue [#3](https://github.com/jordigilh/zvec-grep/issues/3), and the related
entries in `docs/upstream-contribution-log.md`. This issue makes the branch
lineage explicit so the Sense baseline is not mistaken for the owner of the
Graphify parity work.

### Handoff decision

Do not commit the active parity delta or open its PR from
`spike/rust-zg-sense-baseline`. Preserve that branch as historical/recovery
evidence. Create a fresh review branch named, for example,
`spike/four-language-codegraph-graphify-parity`:

1. For fork-stacked review, base it on
   `fork/integration/zvec-live-code-intelligence`, which contains the original
   codegraph/MCP foundation and issue #3 scope.
2. For an upstream PR, port the same logical changes onto the current
   `origin/main` and split the foundational graph feature from the semantic
   producer and parity follow-ups.
3. Apply the uncommitted work there and commit logical review slices rather than
   bundling the unrelated Sense and Code IR histories.
4. Run the complete four-language codegraph gates before opening the matching
   fork or upstream PR.

This handoff is complete: the dedicated branch was created from the updated
integration tip, the parity commits were applied there, and fork PR #24 was
opened. The PR remains draft while the follow-up relation/query work below is
implemented.

The original source branch should not be rewritten or reset; it remains the
provenance copy. The dedicated parity branch is the active review checkout.
