# Controlled synthetic Code IR pilot — decision and evidence

**Decision ([PR #19](https://github.com/jordigilh/zvec-grep/pull/19)):**
Keep existing syntax search as the discovery fallback and the
IR projection **opt-in**. This tiny, source-pinned pilot demonstrates a bounded
syntax→IR *evidence* path across Go, Python, Rust and TypeScript; it does **not**
show a retrieval gain or warrant default ranking. On every lane, the
syntax-only and actual published/read-validated v2 IR indexes find all five
authored answer sites at @10, but there are only **five or six baseline
candidates** (five to seven IR records), so @10 is saturated. For a distinct
Go answer, IR drops from rank 1 to rank 5; TypeScript loses rank on four of
five answers. Do not convert that @10 tie into a quality win.

The new [plan](./code-ir-controlled-synthetic-suite-plan.md) and
`test/fixtures/code-ir-controlled-v1/{go,python,rust,typescript}/` contain two
small files per lane and *source-authored* truth: independently pinned file
hashes, unique literal anchors and their exact UTF-8 byte offsets, expected
declaration kind/name/path, three relationship judgments and five task queries
(two calibration, three predeclared holdout). Same-name decoys, misleading
comments, nested members, cross-file lookalikes, and Go/TypeScript leading
declaration prefixes are included. These are **new** inputs: the four earlier
Engram fixtures and their labels were not changed.

## What was measured, and what was not

An opt-in runner stages *copies* in fresh external directories, indexes them
with the same unchanged local CPU model, engine, hybrid fusion and top-ten
limit, and publishes/reads the v2 projection. The source-byte and model/
snapshot checks run before scoring. A syntax hit is credited **only** when
its label-blind, path/name/kind/range-grounded IR unit matches the authored
answer anchor; an enclosing type is **not** a method hit. IR hits map from
projection record IDs to IR unit IDs, then to the same authored site. Neither
arm is remapped using a line-overlap qrel or reranked after search.

The third, offline, **syntax-discovery→IR-follow-up** arm keeps the syntax
rank unchanged. It grounds to the validated IR without using answer labels;
only then does the evaluator compare the grounded unit/fact to the hand-written
truth. A `contains` fact must have the observed subject/object and source site.
A `calls` fact conveys its **observed source occurrence and unresolved
spelling**, not a bound callee: even a same-named or apparently correct target
is not licensed. A fabricated type-resolved call is explicitly counted as a
false link in offline tests and never returned as a verified answer. Cited
answer/fact bytes are rechecked against the pinned original files.

| Lane | Syntax entities / grounded / IR records | Strict answer ranks for `check`, `structure`, `dispatch`, `decoy`, `proxy` (syntax → IR) | Positive `contains` / unresolved calls / false links |
|---|---|---|---|
| Go | 5 / 5 / 5 | 1→5, 1→1, 2→2, 1→1, 1→1 | 1 / 2 / 0 |
| Python | 5 / 5 / 5 | 2→2, 2→3, 5→5, 1→1, 1→1 | 1 / 2 / 0 |
| Rust | 6 / 6 / 6 | 2→2, 1→1, 6→6, 1→1, 1→1 | 1 / 2 / 0 |
| TypeScript | 5 / 5 / 7 | 2→3, 1→4, 5→7, 1→1, 1→2 | 1 / 2 / 0 |

All four lanes have zero **judged** missing site/relationship records. Rust's
sixth grounded entity is not a labelled answer site; `grounded` is not an
answer-quality score. The `contains` evidence appears on the calibration
structure task only. The eight unresolved-call outcomes are held-out tasks;
the follow-up reports a source site/spelling and **abstains from a target**.
Baseline-only has no attested relationship graph, so a contained child is
additional evidence available through IR, not a baseline ranking improvement.

### Interpretation limits

- **Pilot, not an unbiased held-out quality result.** Splits were declared in
  the truth file before running search, and source/truth/queries were not
  adjusted to improve ranks. However the holdout runs were inspected while
  finalizing the scorer/label-blind grounding contract, so these are *not*
  untouched blind validation. A next suite must freeze the checker first,
  contain >10 credible candidate units and new unseen questions, and report
  errors at smaller cutoffs. Do not tune these five queries or infer a generic
  gain from this saturated @10 pilot.
- The IR's syntactic calls are **unresolved** even where humans can see likely
  targets. No typed bindings, cross-file call correctness, executable behavior,
  data-flow or negative semantic claims were established. The four positive
  facts test containment only; 0 observed false links is on eight *labelled*
  call sites, not a corpus-wide precision guarantee. Source hashes validate
  parsing inputs, not the semantics of prose or the entire IR.
- Grounding deliberately requires a common declaration end byte and compatible
  path/kind/name. It abstains on mismatched ends/anonymous declarations and
  does not show full-corpus coverage. Citation correctness is checked for
  emitted report rows; no default app API or query reranking was changed.
- This is a two-file, parse-only miniature per language. Rust snippets are
  parser examples, not a compiled crate. Real-repository judgments remain a
  later *external-validity* gate, not the next debugging surface.

## Reproduction and provenance

At pinned implementation commit `f9a2d70` (including the label-blind grounding
correction and model-cache integrity check), build and run each language separately:

```sh
npm run build
python3 scripts/replay_code_ir_controlled.py \
  --fixture test/fixtures/code-ir-controlled-v1/go \
  --model-cache /Users/jgil/.engram/zvec-grep/model \
  --work-dir /path/to/FRESH-external-go-work \
  --output-dir /path/to/FRESH-external-go-results
```

Replace `go` with `python`, `rust`, `typescript`, and use a new work/output
pair for each. The output has `raw-runs.json`, `strict-results.json` (all
per-query ranks, abstentions, and source citations), and `run-manifest.json`
(file hashes, model, engine revision, snapshot and implementation digests).
On this host, final artifacts are under the approved temporary
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/` with
`code-ir-controlled-{language}-{work,results}-checked-20260926` names. Temp
artifacts can be cleaned; the runner and the following digests allow replay.
All four runs pinned the same local model-cache tree SHA-256
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`,
verified unchanged before/after each run.

| Language | Truth SHA-256 | Raw search SHA-256 | Strict result SHA-256 |
|---|---|---|---|
| Go | `662fee6f96d89ef6eabee55d6a43540939a2a59c44927eb295dd7914e799b99e` | `0ce0c4dbce3f71f9bb6bb6676ed9f7b224cbd6aa710cbce3bf56b9088584a11d` | `d9da80bdf759c92d43bec4ccefd1710f0d81e9c1ce3c9bd425d9d14b4bd20255` |
| Python | `de4b1895900ad1d07343d222bb6679b047d946fe29cc4a1e72893bebac9c2987` | `e37b5628d14ff595d2002763a86bbdc3efcc5f58da354ed99e8d1b550171864c` | `12ed30b64b19d51644a0d45610028ccb4d9cb60cd3ce362726743578006d3398` |
| Rust | `80bb94bd83cb90c2535156d2cf06673f1298d249eeb7b65b953f4b9e46e06b3c` | `2baa84574b245b360b2275a74e0f3d40b3a31bbe2a35ec894028646c5ee3febf` | `ced431419cee21b6aa3a1a550a41a90cce9cb3a3b467d21e489129bae06b20bd` |
| TypeScript | `4497c9ac1aea85e326907573c20791406e92840d90bc29f3c7aaf15e810c9ea4` | `3c91643518a8d2e8d97b3119b99a1a88407e5f7f79f93dadcdc983a4b7f1d8af` | `a637862839b26b00f8ec65d6d6bdffefb729d8e237d053aa28e37bb58c107467` |

The emitted raw runs use the same search code and model as PR #16; scoring is
new and exact-byte based, not Engram's overlap scorer. `npm run test:unit`
passed **331 / 332** with one existing skip; two offline Python runner tests,
build, typecheck, lint and
format passed. Root/integration/e2e/package/coverage suites and actual compiled
Rust execution were not run. The older Engram fixtures were not inputs or
modified by this pilot. At final status check the separate Engram checkout had
working-tree changes in `src/engram/search/engram.py` and
`tests/test_engram_cocoindex_search.py`; they were left untouched.
