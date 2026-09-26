# Why IR-first retrieval lost Go/Python ground (2026-09-26)

**Verdict:** The frozen Python v1 regression is caused by **search input text,
not missing selected units or changed source windows** in this fixture. On the
same 33 exact-byte IR units, substituting *both* original syntax-index FTS and
vector text reproduces every syntax hit order and per-hit FTS/vector route rank
across all eight queries, with identical aggregate scores. FTS text alone
recovers most, but not all, of the lost nDCG. Go remains **unattributed**: 11
of its 38 matched units have different source byte ranges, so we did not claim
a full-corpus text-only comparison there. This is diagnosis of the frozen
first-stage projection, **not** proof IR-assisted relationship lookup is weak.

## What was held fixed

The [predeclared plan](./code-ir-v1-gap-diagnostic-plan.md) uses the
[PR #17 five-arm replay](./code-ir-projection-ablation-results-20260926.md),
read-only indexed syntax storage and read-validated published v2 sidecars for
each language. A fresh, separate v1 sidecar is generated *outside both source
checkouts* from the SHA-pinned fixture, then read-validated against exactly the
same snapshot/model. Syntax source files, manifests, truth, qrels and Engram's
scorer/source-unit mapper are unchanged. These comparisons use original source
**UTF-8 bytes**, not the scorer's overlapping-line definition of a relevant
unit. The syntax index is an existing extractor, not independent truth for
semantic correctness; only Go has a separate 38-site Go AST inventory.

### Complete indexed-source inventory (not just top-ten hits)

| Lane | Syntax entities → selected IR v2 units | Same name/path/**exact byte span** | Exact bytes, different symbol kind | Name/path match but **range differs** | Other unmatched syntax | Selected IR units without unique syntax match |
|---|---:|---:|---:|---:|---:|---:|
| Go | 38 → 38 | 27/38 (18 same kind + 9 method/function) | 9 | 11 | 0 | 0 |
| Python | 33 → 33 | **33/33** (23 same kind + 10 method/function) | 10 | 0 | 0 | 0 |
| Rust | 42 → 49 | 33/42 (23 same kind + 10 different kind) | 10 | 2 | 7 (6 unnamed file fallbacks, 1 ambiguous struct/`impl` name) | 14 |
| TypeScript | 33 → 38 | 10/33 (different kind) | 10 | 23 | 0 | 5 |

Here “exact bytes” means both endpoints match on the same SHA-pinned file;
the source slice of **every** indexed syntax entity itself round-trips. A
function/method symbol-kind mismatch is **not** a source-span mismatch. Go's
ten short range changes add the leading `type `; `WorkflowState` additionally
includes its preceding doc comment in the IR source span. TypeScript's 23
short changes add leading `export `. Rust has additional unnamed module
fallbacks and an ambiguous struct/`impl` identity. All selected-unit counts
are inventory counts, not assertions about whole-repository parser coverage.

Matching only name/path/source **lines** would misleadingly report 37/38 Go,
33/33 Python, 33/42 Rust and **33/33 TypeScript**; in TypeScript only 10/33
have the same byte span. This is why line-overlap qrel credit cannot stand in
for source-identity conformance.

### Search input audit

For all exact-byte-comparable units, the baseline lexical text differs from
**both** v1 and v2 lexical text (Go 27, Python 33, Rust 33, TypeScript 10).
Baseline signature lines exist in all of those; v1 has zero, and v2 restores
them all. For Python, v2 still emits an empty `scope:` line on 23 top-level
units, labels ten methods `method` where the syntax index labels them
`function`, and formats at least two multiline signatures differently. This
is a concrete, general metadata-policy difference; these observations do not
by themselves assign a score effect to any **one** prefix. Source slices and
metadata are separate from citation authority.

## Controlled Python FTS/vector swap

All 33 source units match byte-for-byte. Before replay, the staged source was
re-extracted with the *existing* `CodeExtractor` and identical token budget;
all 33 fragment IDs, ranges, content and metadata matched stored syntax
entities, and none had an embedding-only window override. We indexed four
views of the **same 33 IR units, source ranges, groups and order** on fresh
collections under the same CPU model, queries, hybrid fusion and scorer:

| Input text on those identical IR units | nDCG@10 | MRR@10 | Recall@10 | Precision@10 |
|---|---:|---:|---:|---:|
| v1 source/base metadata (`code-ir-policy-only`) | 0.482592 | 0.533333 | 0.639583 | 0.225000 |
| Original syntax FTS; v1 vector | 0.535891 | 0.551190 | 0.689583 | 0.250000 |
| v1 FTS; original syntax vector | 0.498765 | 0.522917 | 0.639583 | 0.225000 |
| Original syntax FTS **and** vector | **0.546521** | **0.547917** | **0.689583** | **0.250000** |
| Actual existing syntax index (control) | **0.546521** | **0.547917** | **0.689583** | **0.250000** |
| Published v2 (combined source-backed metadata) | 0.537513 | 0.530357 | 0.689583 | 0.250000 |

With both original inputs, **all eight top-ten path/range orders and all
reported FTS/vector route ranks** also match the syntax index, not just the
averages. This tightly isolates the Python v1 loss to FTS/vector input text
for this corpus; different IR IDs and retained IR relationships did not cause
that loss. Relative to policy-only, FTS restoration adds +0.053299 nDCG and
+0.017857 MRR; vector restoration alone adds +0.016173 nDCG but **loses**
0.010417 MRR. The effects interact; do not add them to predict a combined
score. The five earlier arms reproduced their prior scores exactly in this
fresh eight-arm run.

Two examples from the stored, full-precision `comparison.json`: Python
`preserve-membership-on-retry` loses −0.320 nDCG and −0.400 recall with v1
policy text; restoring syntax FTS shrinks the nDCG loss to −0.036 and restores
recall; restoring both texts removes that query loss. Python v2 still loses
`validate-workflow-parameters` nDCG/MRR (−0.036/−0.083), consistent with the
previously traced fusion-boundary jump by a grade-0 result. **No specific
signature, scope or symbol-type token has been isolated as its cause.** The
unchanged line-overlap scorer can credit an enclosing class hit for a nested
method, and these eight synthetic queries cannot establish real-repository
quality.

## Decision and next experiment

Keep default search unchanged. Do **not** repair retrieval by changing the
canonical byte spans: Go's independently AST-checked IR ranges include source
syntax and attached docs that the syntax retrieval fragment omits. Instead,
test source-preserving, versioned retrieval views: isolate generic lexical
prefix/symbol/signature choices on Python and separately compare Go's
type/doc-bearing projection with a controlled range-aligned view. The other
two languages require their own controls, not a pooled score. Evaluate baseline
discovery → IR-grounded relationship follow-up on independently judged
real-Kubernaut tasks and negative/ambiguous cases before any enablement claim.

## Reproduction and provenance

Run code at `db69ffd` after `npm run build`. With freshly replayed PR #17
five-arm controls and separate external baseline/sidecar storage, for each
language use fresh output; Go example:

```sh
node scripts/diagnose_code_ir_v1_gap.mjs \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --results-dir /path/to/code-ir-v1-gap-control-go-results-20260926 \
  --baseline-index /path/to/code-ir-v1-gap-control-go-work-20260926/baseline/.zvec-grep \
  --sidecar-root /path/to/code-ir-v1-gap-control-go-work-20260926/ir-sidecar \
  --output-dir /path/to/fresh-go-gap-diagnostics
```

Python-only exact-byte text controls (fresh roots outside both checkouts):

```sh
python3 scripts/replay_code_ir_v2_qeval.py --ablation --text-isolation \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/python-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /path/to/fresh-python-text-work \
  --output-dir /path/to/fresh-python-text-results
```

Results on this host are under
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`.
Each of the four `code-ir-v1-gap-verified-{language}-results-20260926/`
directories holds `inventory.json`, `lexical-audit.json`,
`query-transitions.json`, generated/read-validated `generated-v1-sidecar/` and
`diagnostic-manifest.json`. The manifest pins the unchanged frozen source,
qrels, previous raw/scored runs, baseline-index and sidecar trees, model/scorer,
diagnostic scripts and new output hashes.

| Language | `inventory.json` SHA-256 | `lexical-audit.json` SHA-256 |
|---|---|---|
| Go | `a0e1ced584953961130d314518af7824dac4a4f73251360b4b55b142f6c3da90` | `fe5888da4823998271d281af773ba3a50d8cf6b36b759bb05901c258b83a30df` |
| Python | `f5f9ff5dec107c0630cd063f4d1791c9571da83fa5c132602a275e2860f696c6` | `41bf5dbe9db726e8d051b697346591b9178e320a8214f44594ab2bf13dc43852` |
| Rust | `d867348f624bcfe7ba7339cb645514c315c2c866e6202f2f766a516f74d05556` | `d4298902ad284e8074cd5bd79ea76d4f340936890127fa034c2aa00950acec68` |
| TypeScript | `b592b89d1b8c524085e24d937a21a429b3a748a9e713d28c1025a81f092a1aec` | `2160b95d6ff28a2b33a227d0b299caf46b36d83ab1d14f50b4868ed2cb5c45ab` |

The Python fresh eight-arm run is in
`code-ir-v1-text-verified-python-results-20260926/`; its `metrics-k10.json` SHA-256
is `e858039fe86c4f00bbbb09ba828031935062b80195c7ca17c18faef99158823b`;
its `comparison.json` SHA-256 is
`d57cf128d392e49dbfea2a0932586fc6afa1c3188838b507c1a12f4c486074ba`.
The shared unchanged model cache SHA-256 is
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`.

Verification: `npm run build`, `npm run typecheck`, `npm run lint`,
`npm run format:check`, seven offline Python runner tests, JS unit suite
(324 passed, one skipped), four read-only full-index inventory runs, and the
fresh Python eight-arm search replay, and default two-arm Go parity with the
five-arm control and v2 scores. Frozen Engram checkout remained clean;
root/integration/e2e suites and independent real-repository judgments were
not run.
