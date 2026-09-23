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
