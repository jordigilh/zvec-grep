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

## Diagnostic observed; candidate hypothesis frozen before reranker evaluation

The first, externally staged Go diagnosis (engine `0bb943f`, outputs under
`go-retrieval-diagnostic-results-20260926/` in the approved external temp
directory) reproduced all four baseline metrics exactly. Of the 34 positive
qrel sites, 22 map to baseline @10, **10 additional positives** map to the
30-result pool and **two** are still outside @30; none lacks *line-overlap*
eligibility in the syntax index. This is the *existing* Engram source-unit
mapper, which may credit enclosing spans; it is not byte-exact evidence that
all 34 declaration entities are indexed. The 30-result call also changes the
fusion cutoff, so its first ten differ from the control: nDCG .604353,
MRR .726389, recall .714583, precision .2625. The MRR loss means **overfetch
alone fails** the no-regression gate. Its @30 pool is nevertheless the single
chosen generation attempt. No query rewriting or second candidate route.

The only reranking attempt will take those **same 30 indexed entity IDs** and
return ten. It will verify each candidate's stored content against the
original, pinned source substring and file SHA before use. Deterministically
split identifiers by camel case, underscores and non-alphanumerics, lowercase,
remove a fixed English-function-word list and a trailing `s` on tokens longer
than four letters (both query and code). For each query token, IDF is
`1 + ln((31)/(1 + document_frequency_in_30))`. Lexical alignment is a
normalized weighted sum of *presence*, not generated synonyms: name ×2,
source bytes ×1, file path ×0.25, divided by 3.25 times the total query IDF.
Sort lexical matches by this score, resolving ties by the original hybrid
rank and then ID. For candidates with a nonzero lexical alignment, combined
score is `1/(60 + hybrid_rank) + 0.5/(60 + lexical_rank)`; candidates with no
alignment retain just the first term. Resolve combined ties by original
hybrid rank and ID. This is a generic, deliberately single-shot lexical
rerank; it **does not claim semantic understanding** or use IR containment,
callee guesses, qrels, query IDs or truth. Rank trace and identities remain
auditable. Test the policy offline, then run it **once** on this frozen Go
fixture. The earlier exposed Go controlled-v2 suite is a secondary sanity
check only if feasible; it is not an unseen validation set.
