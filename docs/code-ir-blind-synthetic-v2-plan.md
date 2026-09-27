# Second controlled Code IR suite: discriminating retrieval

**Review slice:** `spike/code-ir-blind-synthetic-v2`, child of [PR
#19](https://github.com/jordigilh/zvec-grep/pull/19). This tests the *same*
disk-backed ZVec hybrid discovery and the label-blind, source-verified IR
follow-up; it does not wire an application API or change default ranking.
Keep all existing frozen Engram fixtures and controlled-v1 inputs read-only.

## Precommitted order, with no score-directed edits

1. **Freeze the evaluator first, without v2 fixture data.** Extend the
   versioned truth checker/runner to accept `controlled-{language}-v2`, three
   source files/lane, two calibration and five holdout tasks. Test stale
   bytes, duplicate/ambiguous anchors, forged resolution, missing facts,
   unknown IDs, displaced query order, source citations and path-independent
   grading with offline, invented test objects. Predeclare exact answer @1,
   @3 and @10, reciprocal rank, relation evidence/abstention/false links,
   candidate inventories and per-query ranks. Do not change the existing v1
   policy/results. **Commit the checker** before authoring v2 source/truth.
2. **Author and pin an independent new scenario**, per language: 3 small
   code files with at least 12 *actually indexed* candidate entities, two
   source-authored calibration and five source-authored held-out questions,
   distinct same-name and lexical decoys, one genuinely observed containment,
   two unresolved cross-file lookalike call sites and an ambiguous grounding
   stress case. Source SHA-256, unique literal anchors, exact byte offsets and
   expected path/name/kind are authored from the new source, **not** extracted
   IR or search output. Commit source + truth and check its source hashes before
   querying. No copying answer labels or query text from the prior pilot.
3. **One prospective paired run/language, fresh external paths.** Use the
   unchanged local CPU model, same query/search/fusion engine, the existing
   syntax index and actual published/read-validated v2 projection. Pin model
   cache, implementation and source digests; verify staging, sidecar and
   citations. Map candidates to attested IR *without using oracle anchors*,
   then score exact authored sites. Evidence follow-up never reranks syntax
   discovery: show `contains` only when observed and `calls` only as an
   unresolved source occurrence, never a guessed target. Report all misses,
   collisions and abstentions by language/split. Do not alter fixture,
   queries, truth or ranking based on the held-out results.

**Stop rules:** If actual indexed candidate pool is <=10, or if the harness
cannot uniquely match source evidence, report the gate as failed rather than
inflating @10; do not add queries/distractors after inspection. If a run
fails due to provenance/operational error, retry with fresh paths and record
the failed attempt. Keep old search the fallback even if a synthetic lane
improves. Real-repository judgments remain a later external-validity gate.

Deliverables: checker change with TDD and a frozen commit; separately committed
new four-language source/truth; one read-only replay per lane with decision-first
report and artifact hashes, then a stacked PR. Do not edit another checkout.
