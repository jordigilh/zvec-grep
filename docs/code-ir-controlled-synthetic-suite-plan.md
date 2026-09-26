# Controlled synthetic Code IR evaluation (issue #8)

**Review slice:** `spike/code-ir-controlled-synthetic-suite`, child of [PR
#18](https://github.com/jordigilh/zvec-grep/pull/18). No default indexing,
ranking, API, or SCIP changes. Do not edit the four earlier frozen Engram
fixtures, their source, manifests, truth, or qrels. This is a **new** suite in
`test/fixtures/code-ir-controlled-v1/`, not an amendment to their judgments.

## Question and order of work

Does an opt-in, source-verified IR add useful **bounded follow-up evidence**
after candidate discovery, without inventing semantic links or hiding retrieval
losses? First establish independently authored right/wrong truth, then run the
unchanged search engine on the same source for syntax-only and published v2 IR
discovery. The third arm uses syntax discovery followed by IR grounding and
bounded relationship lookup; it **does not rerank** the syntax results. Score
each language independently. The previous eight-query rankings are not a
development target for these new fixtures.

1. Author two small, ordinary source files per language (Go, Python, Rust,
   TypeScript), with identical-looking names/decoy prose, nested members,
   cross-file unresolved calls, and declaration prefixes (`type`, `export`,
   or doc attachment). Pin SHA-256 per file. In hand-authored truth, uniquely
   identify each site by literal UTF-8 source text, expected path/name/kind,
   and its exact byte range. Do **not** generate judgments from extracted IR,
   hits, or query scores. Predeclare calibration and held-out task IDs.
2. Start with failing offline tests for stale bytes, duplicate/missing anchors,
   wrong owner or enclosing-class credit, same-name decoys, unknown query/site
   IDs, forged call targets, ambiguous grounding, and order-independent
   evaluation. Then implement a read-only verifier/evaluator. Byte offsets
   are `[start,end)`; a source-backed name/kind and an exact authored anchor
   distinguish a method from its enclosing type. Report missing and extra
   evidence; never coerce `unresolved` to `type_resolved`.
3. Build staged copies in *fresh* external directories; validate published
   snapshot and source hashes. Compare syntax-only and IR-v2-only top-10 answer
   retrieval (strict identity, not line overlap). Ground baseline candidate
   entities to verified IR units by path, kind, name and **source-contained
   authored anchor**, rejecting non-unique matches. Follow only `contains`
   facts with `observed` object IDs; show `calls` sites with spelling and
   `unresolved` status, **never a guessed callee**. Score answer retrieval,
   grounded coverage, true positive contains, missed evidence, abstention,
   false links and source citation separately; publish per-query failures.

### Safety/interpretation gates

- Read-only source truth and immutable source hashes are a prerequisite to
  any metric; parser self-consistency is *not* an independent oracle.
- Baseline-only has no attested graph; do not score it as if it produced IR
  relationships. Baseline→IR can add evidence, not improve its first-stage
  rank. Positive/negative links are scored separately from discovery.
- A same-name unresolved call may be an observed site, but the IR provides no
  binding; a claimed callee is always a false link, even if a guessed target
  happens to be correct. Unmatched/ambiguous grounding must abstain.
- Held-out questions may be run only after fixture/truth, checker and retrieval
  mapping are frozen; do not adjust policy, labels, queries, model, or fusion
  based on their scores. No suite win authorizes default ranking. Real-repo
  external validity is a **later** gate, not the next debugging environment.

Deliverables: new pinned fixture/truth, verifier and TDD tests, opt-in same-
engine three-arm runner and per-language decision-first report with artifact
hashes and declared limitations. Keep code and result review commits distinct.
