# Code IR worktree and dependency map

**Last checked: 2026-09-26.** This is the entry point for resuming [issue
#8](https://github.com/jordigilh/zvec-grep/issues/8). The branches listed here
are **separate checkouts of the same fork**, not sequential environments or
automatically merged layers. Commit SHAs below identify implementation bases
and checkpoints; later documentation/evaluation commits can advance a branch.
Run `git status --short --branch` and `git log -1` in the checkout before
making changes.

## Where we are

```text
integration/zvec-live-code-intelligence @ 5f2c5e4  (pre-spike base)
├── spike/code-ir-design @ 139fe4e               (canonical IR contract/frontends)
│   └── spike/code-ir-one-pass-evidence @ 73b424c+ (CURRENT: syntax evidence)
└── spike/code-ir-scip-evaluation @ 3dd88de       (separate experiment + status)

Engram frozen source/manifests/qrels ──read-only inputs──▶ conformance + qeval
                                           └─▶ independent real-repo labels TBD
```

| Role | Branch / observed commit | Checkout on this host | What it contains | Action |
|---|---|---|---|---|
| Pre-spike integration base | `integration/zvec-live-code-intelligence` / `5f2c5e4` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-fork` | Existing search behavior; control ancestry | Do not overwrite for this spike. |
| Owning IR foundation | `spike/code-ir-design` / `139fe4e` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design` | [Normative source/IR contract](./code-ir-design-issue-8.md), [staged implementation plan](./code-ir-implementation-plan-issue-8.md), v1.5 four-language syntax IR, validator, opt-in shadow/projection | Read as base. Leave the other team's checkout clean; integrate the child branch deliberately, not by editing this working tree. |
| **Active syntax follow-up** | [`spike/code-ir-one-pass-evidence`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-one-pass-evidence) / code milestone `73b424c` | `/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/code-ir-one-pass-evidence-20260926` | v1.6 one-parse source-backed signatures/docs, TDD, [results](./code-ir-one-pass-metadata-results-20260926.md) | Continue syntax conformance and retrieval-policy work here. This temp checkout may disappear; **the pushed fork branch is durable**. |
| Evaluation/design reference | [`spike/code-ir-scip-evaluation`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-scip-evaluation) / `3dd88de` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-scip-spike` | [Semantic-index design](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-scip-evaluation/docs/code-ir-semantic-index-design.md), [living work status](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-scip-evaluation/docs/code-ir-scip-work-status.md), earlier four-language retrieval ablations and offline relationship lookup | Read-only evidence for this syntax milestone. Do **not** treat its experimental producer artifacts as a dependency of the base IR. |

The older `zvec-grep-lexical-baseline`, `sense-evidence` and
`proposal-{1,2,3,4}` checkouts are prior retrieval controls/ablations, **not**
branches to merge into the current implementation. The frozen evaluation
fixtures belong to the separate `engram/benchmarks/semantic_search/fixtures/`
checkout. Never edit their source, manifests, truth or qrels; use copied/staged
inputs for builds. The isolated local upstream-producer worktree is also not
part of the active task. There is no merged PR or default search change from
these branches.

## What depends on what

1. **Source/IR conformance comes first.** Schema, byte validator and syntax
   frontends are owned by `spike/code-ir-design`; v1.6 source metadata is a
   child branch, not yet integrated into the owning branch or default engine.
   Validate exact source round trips and independently authored positive,
   negative and missing cases **per language**. The already emitted unit/fact
   IDs are unchanged in the four frozen lanes, but emitted metadata counts do
   not establish whole-corpus completeness.
2. **Derived retrieval is separate.** The existing opt-in projection can
   consume only an attested IR snapshot. A new versioned policy must decide
   which units rank independently (while retaining fine-grained IR targets) and
   whether source-backed signatures/docs belong in FTS/vector input. Compare
   fresh candidate indexes to an unchanged, same-engine control on *each*
   language's frozen qrels; Go/Python currently fail the aggregate gate and
   Rust/TypeScript have per-query losses. No cross-language average or default
   rollout follows from a schema improvement.
3. **Real-repository quality is another gate.** Kubernaut needs independently
   judged retrieval qrels and negative/ambiguous relationship sites. A broad
   source-map-valid index is not a relevance or semantic-precision score.
4. **Optional semantic producers are deferred.** The syntax IR, retrieval
   projection and their gates do not wait for scip-go or any other semantic
   producer. Bounded relationship lookup and external bindings remain an
   independent, opt-in experiment.

## Next handoff on this branch

- **Now:** Build a small *independent* metadata inventory from frozen source
  bytes, with exact expected header/comment slices and explicit absent/comment
  negatives for all four languages. Test the inventory's path/hash and
  uniqueness invariants before comparing it with v1.6 extraction. Report the
  selected-case denominator rather than claiming full fixture coverage.
- **Then:** Add an explicit, versioned retrieval-unit policy and source-ref
  projection in an opt-in path; drive it with tests before paired per-language
  qeval. Keep source citations on original bytes and preserve the default
  syntax-only fallback.
- **Later:** Independently adjudicate real-Kubernaut retrieval and relation
  cases before generalizing or enabling anything by default.

**Resume checklist:** start with this map and the [one-pass evidence
report](./code-ir-one-pass-metadata-results-20260926.md); check both active
branch heads and `git status`; read the owning IR contract; run focused tests;
then consult issue #8 and the evaluation branch for unchanged qrels and
historical metrics. Commit in logical groups and push only the branch where the
work was done. Do not edit other worktrees to "sync" them; integration is an
explicit future review step.
