# Go-only retrieval timebox (paused four-language IR work)

**Scope:** The four-language IR experiment is parked, with its last clean,
pushed review slice at [PR #20](https://github.com/jordigilh/zvec-grep/pull/20).
This Go-only branch starts from that slice to reuse the pinned evaluation
harness; it is **not** an extension of the IR schema and changes no default
search or ranking. Do not edit the Engram frozen source/manifest/truth/qrels,
the controlled-v2 Go source/truth, or the previous result reports. Keep fork
`main` untouched. Record the dependency if a separate PR is opened; a useful
result can later be extracted closer to the integrated baseline.

## Decision question and budget

Does **one** bounded candidate-generation change and **one** generic,
source-evidence reranker improve the existing Sense-enhanced **syntax-only**
Go retrieval baseline? No language-specific rules for known query IDs, answer
symbols, source paths or qrel labels. This timebox is *one diagnosis, at most
one prespecified candidate-generation arm, at most one prespecified reranker
on that pool, and one paired replay* against the unchanged baseline. If they
fail, report the loss and stop: no parameter sweeps or after-the-fact edits.

1. **Diagnose before choosing a strategy.** Reindex the read-only Go workflow
   fixture with the original local CPU model and same engine. Verify that the
   10-result control matches 0.588774 nDCG / 0.733333 MRR / 0.677083 recall /
   0.250000 precision @10 to 1e-6. Inventory each positive source unit at
   @10 and @30, distinguishing *not in index*, *not recalled into @30*, and
   *present but poorly ordered*. Snapshot hashes, entity identities, source
   ranges and route traces must be saved. The frozen Go questions have been
   inspected in earlier work: **this is exploratory development**, not an
   unseen held-out test.
2. **Precommit the two hypotheses after diagnosis, before scoring them.**
   Candidate generation is limited to top-30 from the same hybrid FTS/vector
   index or one reproducible, label-blind additional query route; never
   qrel-driven injection. Reranking may use only query text, source-verified
   candidate bytes and existing index metadata/trace, never answer labels,
   external bindings or fake callee edges. If @30 already contains every
   positive, use the top-30 overfetch as the sole generation arm; avoid an
   additional query-expansion variant. Record formulas/tie breaks, limits,
   and failure behavior **before reading candidate metrics**.
3. **TDD then one evaluation.** Offline tests must prove identity/deduping,
   stable ordering, source-hash/range validation, unchanged top-ten control,
   no label access by generation/ranking, no invented positive, exactly ten
   returned results, and fail-closed on stale bytes. Score the original Go
   qrels with the existing Engram mapper/scorer and unaltered model on fresh
   external work/output paths; optionally use the already-exposed controlled-v2
   Go exact-byte suite as a *secondary stress check*, never as a tuning target.

**Go workflow acceptance (exploratory, not rollout):** Baseline @10 must
reproduce exactly. Candidate pool @30 must contain at least one relevant unit
previously absent from @10 to justify reranking. In the final @10 result,
nDCG, MRR, recall and precision must have **no negative aggregate deltas**
(1e-12 tolerance) and at least one must strictly improve. Report every
query-level loss even if aggregate passes. Any uncontrolled difference in
source bytes, qrels, model, evaluator or rank mapping invalidates the run.
Even a pass here requires **new independently judged, truly unseen Go tasks**
and latency/operations evidence before proposing product enablement; do not
expand to Python/Rust/TS until that independent Go gate succeeds. Stop after
the single prespecified attempt regardless of outcome.
