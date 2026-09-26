# Code IR worktree and dependency map

**Last checked: 2026-09-26.** This is the entry point for resuming [issue
#8](https://github.com/jordigilh/zvec-grep/issues/8). The branches listed here
are **separate checkouts of the same fork**, not sequential environments or
automatically merged layers. Commit SHAs below identify observed merge and
checkpoint states; later work can advance a branch.
Run `git status --short --branch` and `git log -1` in the checkout before
making changes.

## Where we are

```text
5f2c5e4 (pre-spike fork integration base)
├── integration/zvec-live-code-intelligence @ acb3abc (PRs #9–#12 MERGED)
│   └── spike/code-ir-metadata-coverage (PR #14: OPEN)
│       └── spike/code-ir-projection-policy-v2 (CURRENT branch/checkout)
└── spike/code-ir-scip-evaluation @ df4a856 (separate experiment + status)

spike/code-ir-metadata-inventory-wip @ b1d4e5a = old unverified backup;
its two files were carried onto a NEW branch from the merged integration tip.

Engram frozen source/manifests/qrels ──read-only inputs──▶ conformance + qeval
                                           └─▶ independent real-repo labels TBD
```

| Role | Branch / observed commit | Checkout on this host | What it contains | Action |
|---|---|---|---|---|
| **Merged Code IR base** | `integration/zvec-live-code-intelligence` / `acb3abc` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-fork` | [Contract](./code-ir-design-issue-8.md), [staged plan](./code-ir-implementation-plan-issue-8.md), opt-in IR/projection and v1.6 source metadata (PRs #9–#12) | Base new PRs on this branch, not the rewritten historical spike heads. Fork `main` remains a separate integration decision. |
| Coverage review | [`spike/code-ir-metadata-coverage`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-metadata-coverage) / based on `acb3abc` | No longer checked out; PR head in same clone | Source-authored labels, verifier tests and [per-lane report](./code-ir-metadata-coverage-results-20260926.md) | Review [PR #14](https://github.com/jordigilh/zvec-grep/pull/14) against merged integration; do not edit frozen fixture repositories. |
| **Active projection review** | `spike/code-ir-projection-policy-v2` / based on #14 | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-fork` | [Versioned opt-in v2 policy](./code-ir-projection-v2-plan.md), source-backed retrieval text, per-language source-only recheck | Open a child PR against #14; do not enable default ranking. |
| Historical IR checkout | `spike/code-ir-design` / local `139fe4e` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design` | Earlier IR authoring branch; merged through #11 with rewritten remote commits | Leave untouched; use integration for current code. |
| Inventory backup | [`spike/code-ir-metadata-inventory-wip`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-metadata-inventory-wip) / `b1d4e5a` | No separate checkout | Original unverified script/test snapshot | Preserved, no PR; the active branch reuses those two files without inheriting the old pre-merge history. |
| Historical one-pass checkout | `spike/code-ir-one-pass-evidence` / local `e96bec9` | `/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/code-ir-one-pass-evidence-20260926` | Original PR #12 worktree; remote was rewritten by the merge flow | Leave untouched. The merged integration tip is authoritative. |
| Evaluation/design reference | [`spike/code-ir-scip-evaluation`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-scip-evaluation) / `df4a856` | `/Users/jgil/go/src/github.com/jordigilh/zvec-grep-scip-spike` | [Semantic-index design](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-scip-evaluation/docs/code-ir-semantic-index-design.md), [living work status](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-scip-evaluation/docs/code-ir-scip-work-status.md), earlier four-language retrieval ablations and offline relationship lookup | Read-only evidence for this syntax milestone. Do **not** treat its experimental producer artifacts as a dependency of the base IR. |

The older `zvec-grep-lexical-baseline`, `sense-evidence` and
`proposal-{1,2,3,4}` checkouts are prior retrieval controls/ablations, **not**
branches to merge into the current implementation. The frozen evaluation
fixtures belong to the separate `engram/benchmarks/semantic_search/fixtures/`
checkout. Never edit their source, manifests, truth or qrels; use copied/staged
inputs for builds. The isolated local upstream-producer worktree is also not
part of the active task. PRs #9–#12 landed on **fork integration**, not fork
`main` or the upstream project; no default search/ranking change was enabled.

## Review stack on the fork

The following PRs were merged **in order onto the fork integration branch**.
GitHub rewrote the old spike branch heads when merging; do not reset local
historical worktrees to their new remote tips. The integration branch already
had five pre-spike commits not on fork `main`; landing the complete integration
branch on `main` is still a separate review decision.

| Merge order | PR | Integration merge commit | Scope |
|---|---|---|---|
| 1 | [#9: evidence IR contract](https://github.com/jordigilh/zvec-grep/pull/9) | `40ce934` | Design and staged plan. |
| 2 | [#10: four-language IR foundation](https://github.com/jordigilh/zvec-grep/pull/10) | `90df925` | Schema, extractors, validators, conformance examples. |
| 3 | [#11: opt-in snapshots](https://github.com/jordigilh/zvec-grep/pull/11) | `ace9b59` | Publication, source refinements and rechecks. |
| 4 | [#12: one-pass source metadata](https://github.com/jordigilh/zvec-grep/pull/12) | `acb3abc` | v1.6 metadata and its TDD/result notes; historical full root-suite timeout remains unverified. |

The independent metadata inventory was **not in #12**. Its original WIP
script/test are preserved on a pushed backup branch; the current coverage branch
starts afresh from merged integration, not that branch's pre-merge ancestry.
The earlier SCIP evaluation is historical evidence, not a hidden fifth
prerequisite.

## What depends on what

1. **Source/IR conformance comes first.** Schema, byte validator and v1.6 syntax
   frontends are merged in opt-in form on integration, but not enabled in the
   default engine.
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

## Next handoff

- **Now (on `spike/code-ir-projection-policy-v2`):** Review [PR
  #14](https://github.com/jordigilh/zvec-grep/pull/14) first, then the
  [versioned opt-in projection](./code-ir-projection-v2-plan.md). The authored
  coverage inventory reports selected-case denominators only; v2 unit counts
  alone do not establish whole-fixture relevance or metadata coverage.
- **Then (separate gate):** Run paired, unchanged same-engine frozen qeval per
  language for v2 and diagnose per-query losses. Keep citations on original
  bytes and preserve default syntax-only fallback. Do not enable failed lanes.
- **Later:** Independently adjudicate real-Kubernaut retrieval and relation
  cases before generalizing or enabling anything by default.

**Resume checklist:** start with this map, the [one-pass evidence
report](./code-ir-one-pass-metadata-results-20260926.md) and the coverage
report; check active branch head and `git status`; read the IR contract; run
focused tests; then consult issue #8 and the evaluation branch for unchanged
qrels and historical metrics. Commit in logical groups and push only the branch
where the work was done. Do not edit other worktrees to "sync" them; integrating
fork `main` is an explicit future review step.
