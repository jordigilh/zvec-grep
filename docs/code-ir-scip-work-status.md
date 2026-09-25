# Code IR + SCIP work status and resume point

**Checkpoint: 2026-09-25.** This is the living handoff for the isolated SCIP
evaluation branch `spike/code-ir-scip-evaluation` in `jordigilh/zvec-grep`.
The measured code and reports preceding this checkpoint end at `282deb5`.
Update the prior experiment commit, upstream PR head, evidence and next step
when work resumes; earlier result reports remain immutable records of the
producer and fixtures actually evaluated.

## Plan and ownership

- [Issue #8](https://github.com/jordigilh/zvec-grep/issues/8) is the project
  objective and decision tracker: a language-neutral, versioned, source-mapped
  Code IR for Go, Python, Rust and TypeScript, with source bytes as evidence and
  retrieval projections kept separate from canonical facts.
- The [Code IR design](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-design/docs/code-ir-design-issue-8.md)
  and [staged implementation plan](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-design/docs/code-ir-implementation-plan-issue-8.md)
  belong to the separate `spike/code-ir-design` checkout (observed at `139fe4e`).
  Its phase 3a defines optional, non-blocking SCIP enrichment after syntax IR
  conformance. Do not edit that team's checkout from this spike.
- This branch's [SCIP evaluation brief](./scip-code-ir-evaluation-brief.md)
  sets the producer/source-map questions. The [experiment report](./scip-code-ir-spike-results.md),
  [retrieval recheck](./code-ir-source-backed-projection-recheck.md),
  [relationship lookup](./scip-shadow-relationship-query.md), and
  [upstream revision revalidation](./scip-go-pr298-revalidation-20260925.md)
  hold the detailed measurements and reproduction paths.

## What has been established

1. The committed syntax-backed Code IR v1.5 frontend and an **optional** SCIP
   shadow can be joined against the same frozen source snapshot. The revised
   `scip-go` PR #298 build at `c1926a3` preserves all 471 primary Go sites,
   strictly joins 117/117 selected-local candidates and produces the same
   218-fact Code IR shadow as the earlier PR build. Independent selected Go
   source labels resolve 20/20 positive sites; the three chosen external or
   ambiguous sites abstain. These are **selected controls**, not precision over
   all 321 raw Go references or a claim that references are calls.
2. A read-only, bounded relationship lookup selects an IR unit or exact byte
   site and returns source-anchored incoming/outgoing `references` after
   rechecking the whole selected file set, config, Code IR implementation,
   shadow and SCIP artifact (and Go producer binary when supplied). Python,
   Rust and TypeScript shadows also pass lookup smoke checks; their selected-
   local fact populations are 49, 58 and 47 respectively. This is an offline
   query surface, not a search-rank change or a production runtime integration.
3. On immutable Kubernaut revision
   `9dcc3816d0eeb75174b86b5f45e4c7d5efc95c17`, the revised Go producer
   keeps all 498,542 primary site/role/symbol tuples on 1,070 documents and
   raises valid definition enclosures from 30,559 to 30,926 (+367). This tests
   producer/source-map coverage; it is **not** a real-Kubernaut IR relationship
   precision or retrieval assessment.
4. The four frozen eight-query, same-engine qeval lanes remain separate.
   Source-backed metadata and entity-granularity parity nearly recover Go's
   syntax-only control, but Go and Python **fail** the aggregate no-regression
   gate; Rust and TypeScript pass aggregate with query-level losses. Flat SCIP
   target-name injection worsens Go retrieval. No default search, graph ranking
   or embedding policy was enabled by this spike.

## Upstream dependency and open decisions

[scip-go PR #298](https://github.com/scip-code/scip-go/pull/298) currently has
head `c1926a380beef870dde29ded54e4f8655bb875f9`: checks passed, but the
review decision was `CHANGES_REQUESTED` and merge was `BLOCKED` at this
checkpoint. The author has [asked the maintainer about three additional
coverage gaps](https://github.com/scip-code/scip-go/pull/298#issuecomment-5839828937):
generic type parameters; block-local type/const/var and anonymous-struct
fields; and sharing a grouped `GenDecl` doc comment across applicable specs.
We support a focused follow-up with separate commits and source-span fixtures,
subject to upstream scope/review. No new PR head implementing those gaps had
been observed at this checkpoint. Rebuild and remeasure any later head; never
reuse `c1926a3`'s binary/index/shadow hashes as evidence for it.

## Resume in this order

1. Check issue #8, the owning design branch and PR #298's latest head/review.
   If the producer changes, build/test the new upstream head in its own checkout
   and rerun [`compare_scip_go_revisions.py`](../scripts/compare_scip_go_revisions.py)
   on identical frozen Go and immutable Kubernaut sources. Audit added,
   removed and changed enclosures **and** the complete primary-site multiset;
   source-author a few positive/negative sites for each new construct. Rebuild
   the strict shadow and compare full canonical facts with
   [`compare_shadow_factsets.py`](../scripts/compare_shadow_factsets.py).
2. Extend independently judged declaration and relationship inventories,
   including false/ambiguous/external cases and real Kubernaut sites. Do not
   infer whole-repository precision from 20 selected positive labels or SCIP
   producer output alone.
3. In the owning Code IR frontend, derive verified signature and contiguous
   documentation source refs in the **same read/parse** as units. Keep field/
   attribute units available as semantic targets while selecting answerable
   retrieval units by an explicit, versioned projection policy. Add complete
   dependency/build configuration to the producer snapshot identity before
   considering a runtime relationship query interface.
4. Build independently adjudicated real-Kubernaut retrieval qrels and rerun
   same-engine, per-language paired baselines with per-query deltas; keep
   extraction correctness, retrieval quality and agent-answer usefulness as
   separate gates. No production-default enablement before those gates pass.

**Reproduction guardrails:** Preserve the frozen fixture source, manifests,
truth and qrels. Regenerate native SCIP binaries in new temporary roots; they
are not checked into git and their digests can differ across producer versions
and runs. Pin source set, producer binary, index, shadow, Code IR implementation
and configuration per experiment. The issue and design state the objectives;
the linked dated reports state what was actually measured.
