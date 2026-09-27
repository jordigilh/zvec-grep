# Source-pinned Code IR retrieval-view ablation (2026-09-26)

**Decision:** No change to default search. Of five same-engine arms, no IR view
passes the aggregate no-regression gate against syntax-only on **Go or Python**.
Rust and TypeScript IR arms pass that synthetic *aggregate* gate, but retain
query losses and have no independently judged real-repository retrieval cases.
No lane is approved for default enablement. SCIP is not used.

## Contract and controls

This is the first, prespecified [2×2 ablation](./code-ir-projection-ablation-plan.md)
on top of the [PR #16 paired replay](./code-ir-v2-frozen-qeval-results-20260926.md):

| Arm | Eligible records | FTS/vector input |
|---|---|---|
| `syntax-only` | Existing syntax extractor/index (unchanged) | Existing search text |
| `code-ir-v1` | Non-file, non-opaque records from published/read-validated v1 | v1 base metadata + source |
| `code-ir-policy-only` | v1 records restricted to actual v2 selected units | v1 base metadata + source |
| `code-ir-metadata-only` | v1 eligible records; v1-only fields/attributes retain v1 text | v2 text for shared units, v1 text for v1-only units |
| `code-ir-v2` | Actual published/read-validated v2 records | v2 source-backed signature + documentation metadata and source |

This is **unit selection versus the combined signature/documentation text**,
not signature-versus-doc, lexical-versus-vector, or window-length isolation.
The four IR views use the same immutable snapshot and independently read-validated
v1/v2 sidecars. All shared records match ordered unit/window keys, full source
and unit refs, and grouping IDs before indexing; any mismatch aborts. Their
source windows are identical in these four fixtures. Go removes 20 standalone
struct fields (58 → 38 eligible records); Python removes 12 class attributes
(45 → 33). Rust and TypeScript select the same 49 and 38 eligible records in
both policies, respectively. All removed units remain in the canonical IR.
`code-ir-v1` is the **eligible subset**, not the full v1 publication or the
default syntax index.

Every arm has fresh, separate index storage and the same search engine, frozen
source bytes, eight source-authored queries/language, unchanged Engram qrels,
source-unit line-overlap mapper and adjudicated scorer, zvec 0.7.0, and cached
CPU `local/potion-code-16m-v2` model. Nothing wrote to Engram's fixture sources,
manifests, truth or qrels. Baseline and v2 metrics exactly match the earlier
two-arm run in **all four languages**; rerunning the default two-arm path with
`--ablation` absent also reproduced its original metrics exactly (Go checked).

## Aggregates, separately by language

Each cell is **nDCG / MRR / Recall / Precision @10** (six decimals). “Pass”
below means at least one aggregate improves without regression relative to
syntax-only; it is **not** a rollout decision.

| Language | Syntax-only | IR v1 eligible | Policy only | Text only | Actual v2 | IR gate versus syntax |
|---|---|---|---|---|---|---|
| Go | 0.588774 / 0.733333 / 0.677083 / 0.250000 | 0.489163 / 0.718750 / 0.543750 / 0.200000 | 0.550753 / 0.781746 / 0.683333 / 0.250000 | 0.557677 / 0.733333 / 0.641667 / 0.237500 | 0.577130 / 0.733333 / 0.677083 / 0.250000 | **All fail** (nDCG lower in each) |
| Python | 0.546521 / 0.547917 / 0.689583 / 0.250000 | 0.492982 / 0.531250 / 0.622917 / 0.225000 | 0.482592 / 0.533333 / 0.639583 / 0.225000 | 0.530904 / 0.524107 / 0.647917 / 0.237500 | 0.537513 / 0.530357 / 0.689583 / 0.250000 | **All fail** (nDCG/MRR lower in each) |
| Rust | 0.416330 / 0.455556 / 0.560417 / 0.200000 | 0.441125 / 0.465774 / 0.622917 / 0.225000 | 0.441125 / 0.465774 / 0.622917 / 0.225000 | 0.459189 / 0.455853 / 0.647917 / 0.237500 | 0.459189 / 0.455853 / 0.647917 / 0.237500 | All pass synthetic aggregates; v2 has **two query losses** |
| TypeScript | 0.426791 / 0.591667 / 0.550000 / 0.200000 | 0.460006 / 0.600000 / 0.616667 / 0.225000 | 0.460006 / 0.600000 / 0.616667 / 0.225000 | 0.465124 / 0.593056 / 0.633333 / 0.225000 | 0.465124 / 0.593056 / 0.633333 / 0.225000 | All pass synthetic aggregates; v2 has **one query loss** |

The effects **depend on the other factor**; do not add one-factor improvements
as though they were independent. Example: Go's v2 selection changes nDCG by
**+0.061591** with v1 text but only **+0.019453** with v2 text. Adding combined
v2 metadata changes Go nDCG by +0.068514 with v1 units, +0.026377 with v2
units; the latter loses MRR by −0.048413. Python's selection loses nDCG
−0.010389 with v1 text but gains +0.006610 with v2 text. Rust/TypeScript have
no selection effect in these fixtures; their metadata-only and v2 runs agree
exactly. Full-precision factor contrasts for **all four metrics and each query**
are in `comparison.json` (see artifacts).

## Every query versus the syntax-only control

Each cell is **ΔnDCG / ΔMRR / ΔRecall / ΔPrecision @10** for its named arm
minus the *same language's* syntax-only control. Values rounded to three
decimals; consult `comparison.json` for exact values and raw route/rank traces.
These are source-authored synthetic qrels, not independent real-repository
judgments; the source-unit mapper credits any overlapping manifest lines.

### Go

| Query | v1 ΔN/M/R/P | policy ΔN/M/R/P | text ΔN/M/R/P | v2 ΔN/M/R/P |
|---|---|---|---|---|
| record-discovered-membership | +0.013 / +0.000 / +0.250 / +0.100 | +0.035 / +0.000 / +0.250 / +0.100 | +0.069 / +0.000 / +0.250 / +0.100 | +0.069 / +0.000 / +0.250 / +0.100 |
| reject-undiscovered-workflow | −0.191 / −0.200 / −0.250 / −0.100 | −0.042 / −0.089 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| forward-labels-to-discovery | −0.190 / +0.000 / +0.000 / +0.000 | −0.382 / +0.000 / +0.000 / +0.000 | −0.004 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| preserve-membership-on-retry | −0.031 / +0.083 / −0.200 / −0.100 | −0.075 / −0.024 / −0.200 / −0.100 | −0.009 / +0.000 / +0.000 / +0.000 | −0.003 / +0.000 / +0.000 / +0.000 |
| interactive-selection-guard | −0.055 / +0.000 / +0.000 / +0.000 | −0.063 / +0.000 / +0.000 / +0.000 | −0.055 / +0.000 / +0.000 / +0.000 | −0.125 / +0.000 / −0.250 / −0.100 |
| validate-workflow-parameters | −0.080 / +0.000 / −0.333 / −0.100 | +0.275 / +0.500 / +0.000 / +0.000 | −0.131 / +0.000 / −0.333 / −0.100 | +0.000 / +0.000 / +0.000 / +0.000 |
| empty-discovery-fails-closed | −0.166 / +0.000 / −0.200 / −0.100 | −0.045 / +0.000 / +0.000 / +0.000 | −0.089 / +0.000 / −0.200 / −0.100 | −0.033 / +0.000 / +0.000 / +0.000 |
| copy-selected-workflow-details | −0.097 / +0.000 / −0.333 / −0.100 | −0.006 / +0.000 / +0.000 / +0.000 | −0.029 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |

### Python

| Query | v1 ΔN/M/R/P | policy ΔN/M/R/P | text ΔN/M/R/P | v2 ΔN/M/R/P |
|---|---|---|---|---|
| record-discovered-membership | −0.178 / +0.000 / +0.000 / +0.000 | −0.178 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| reject-undiscovered-workflow | +0.008 / +0.083 / +0.000 / +0.000 | −0.014 / +0.083 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| forward-labels-to-discovery | −0.088 / −0.250 / +0.000 / +0.000 | +0.004 / +0.000 / +0.000 / +0.000 | +0.017 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| preserve-membership-on-retry | −0.320 / −0.200 / −0.400 / −0.200 | −0.320 / −0.200 / −0.400 / −0.200 | −0.036 / −0.057 / +0.000 / +0.000 | −0.036 / −0.057 / +0.000 / +0.000 |
| interactive-selection-guard | −0.003 / +0.000 / +0.000 / +0.000 | −0.003 / +0.000 / +0.000 / +0.000 | −0.003 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| validate-workflow-parameters | +0.065 / +0.167 / +0.000 / +0.000 | −0.002 / +0.000 / +0.000 / +0.000 | −0.016 / −0.133 / +0.000 / +0.000 | −0.036 / −0.083 / +0.000 / +0.000 |
| empty-discovery-fails-closed | +0.182 / +0.067 / +0.200 / +0.100 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| copy-selected-workflow-details | −0.096 / +0.000 / −0.333 / −0.100 | +0.000 / +0.000 / +0.000 / +0.000 | −0.087 / +0.000 / −0.333 / −0.100 | +0.000 / +0.000 / +0.000 / +0.000 |

### Rust

| Query | v1 ΔN/M/R/P | policy ΔN/M/R/P | text ΔN/M/R/P | v2 ΔN/M/R/P |
|---|---|---|---|---|
| record-discovered-membership | +0.206 / +0.000 / +0.500 / +0.200 | +0.206 / +0.000 / +0.500 / +0.200 | +0.305 / +0.000 / +0.500 / +0.200 | +0.305 / +0.000 / +0.500 / +0.200 |
| reject-undiscovered-workflow | +0.104 / +0.233 / +0.000 / +0.000 | +0.104 / +0.233 / +0.000 / +0.000 | +0.022 / +0.043 / +0.000 / +0.000 | +0.022 / +0.043 / +0.000 / +0.000 |
| forward-labels-to-discovery | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| preserve-membership-on-retry | −0.137 / −0.111 / −0.200 / −0.100 | −0.137 / −0.111 / −0.200 / −0.100 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| interactive-selection-guard | −0.014 / +0.000 / +0.000 / +0.000 | −0.014 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 |
| validate-workflow-parameters | −0.040 / −0.083 / +0.000 / +0.000 | −0.040 / −0.083 / +0.000 / +0.000 | −0.045 / −0.083 / +0.000 / +0.000 | −0.045 / −0.083 / +0.000 / +0.000 |
| empty-discovery-fails-closed | +0.079 / +0.043 / +0.200 / +0.100 | +0.079 / +0.043 / +0.200 / +0.100 | +0.071 / +0.043 / +0.200 / +0.100 | +0.071 / +0.043 / +0.200 / +0.100 |
| copy-selected-workflow-details | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |

### TypeScript

| Query | v1 ΔN/M/R/P | policy ΔN/M/R/P | text ΔN/M/R/P | v2 ΔN/M/R/P |
|---|---|---|---|---|
| record-discovered-membership | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| reject-undiscovered-workflow | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| forward-labels-to-discovery | −0.009 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 |
| preserve-membership-on-retry | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| interactive-selection-guard | −0.010 / +0.000 / +0.000 / +0.000 | −0.010 / +0.000 / +0.000 / +0.000 | +0.012 / +0.000 / +0.000 / +0.000 | +0.012 / +0.000 / +0.000 / +0.000 |
| validate-workflow-parameters | +0.197 / +0.000 / +0.333 / +0.100 | +0.197 / +0.000 / +0.333 / +0.100 | +0.298 / +0.000 / +0.667 / +0.200 | +0.298 / +0.000 / +0.667 / +0.200 |
| empty-discovery-fails-closed | +0.087 / +0.067 / +0.200 / +0.100 | +0.087 / +0.067 / +0.200 / +0.100 | +0.005 / +0.011 / +0.000 / +0.000 | +0.005 / +0.011 / +0.000 / +0.000 |
| copy-selected-workflow-details | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |

## What the traces do and do not explain

- Go `interactive-selection-guard`: relevant, independently byte-verified
  `DiscoveryResult` appears at raw rank 9 with both syntax and policy-only
  but outside @10 with metadata-only and v2. The combined metadata text is
  implicated in its displacement, **not** a missing mapped declaration.
- Python `validate-workflow-parameters`: grade-0 `apply_selected_workflow`
  has FTS/vector route ranks 4/1 in policy-only (raw rank 6) and 7/2 in
  metadata-only (raw rank 7). **Together**, selection and text produce route
  ranks 5/2 and raw rank 1: the 2.5 rank-ratio corroboration boundary in the
  *unchanged* fusion engine is crossed. Neither single-factor arm yields this
  jump; this is an interaction, not proof that a specific signature or doc
  token caused it. Python `preserve-membership-on-retry`'s grade-0
  `WorkflowState.contains` already rises to raw rank 4 in v1 and policy-only
  versus syntax rank 9; v2 rank 3 is not wholly explained by v2 metadata.
- Rust/TypeScript differences in these fixtures are entirely from the combined
  text factor (eligible unit policy selects the same units). Their query losses
  and Python/Go's aggregate failures forbid default ranking changes.

Eight queries per language are too small to estimate real-repository quality;
per-query losses are visible, but the source-unit line-overlap mapper can credit
an enclosing Python class or Rust `impl` for a nested method. Only Go's 38
selected sites have an independent exact declaration byte-span/kind/token
inventory. This experiment neither establishes exact-span coverage on the
other languages nor validates negative relationship assertions.

**Next controlled experiments:** separately ablate signature versus documentation,
lexical versus vector text and source-window boundaries (abstaining on unmatched
source refs); then evaluate fusion-band sensitivity against general, separately
judged queries without fitting the eight frozen queries. Independently label
real-Kubernaut retrieval and negative/ambiguous relationship cases. Keep the
existing discovery fallback while these gates are open.

## Reproduction and artifact hashes

Run committed adapter/code at `66bd334` after `npm run build`; example Go
(repeat for `python`, `rust`, `typescript` with fresh output paths):

```sh
python3 scripts/replay_code_ir_v2_qeval.py --ablation \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /path/to/fresh-factorial-go-work \
  --output-dir /path/to/fresh-factorial-go-results
```

The runner refuses preexisting work/output and checkout-local artifact paths.
Each result directory contains `raw-runs.json`, `normalized-runs.json`,
`metrics-k10.json`, `comparison.json`, and `run-manifest.json` with full source,
fixture/query, model, scorer, mapping, adapter, runner, ablation, engine-dist,
and artifact hashes. All runs used model-cache tree SHA-256
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`;
the unchanged Engram source digests agree with the [previous report](./code-ir-v2-frozen-qeval-results-20260926.md).
The committed ablation implementation SHA-256 is
`e64bd395dd8b34fe58eb21a56061a042a3ccd6926ddfa6b0dc084e8922dfb122`.

External artifacts on this host, under
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`:

| Language | Result directory | Frozen source SHA-256 | `metrics-k10.json` SHA-256 | `comparison.json` SHA-256 |
|---|---|---|---|---|
| Go | `code-ir-factorial-pinned-go-results-20260926/` | `94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f` | `0cb0af6df9dc1844c4bb2ebade209621bd584302bdd3f92d1b55aff2ed56792e` | `72ae4e2028bf050fe483853455d51d537d2c35832643fd8502c0f27ea77c6406` |
| Python | `code-ir-factorial-pinned-python-results-20260926/` | `0b77b06131e0a0e8ffbafa258c54e8900b7c32c2ea6f156ea75e8c6a181e076b` | `f551bd9d2272e47de34d99fc1e5b679794eda6ba55ab84db5c1e738bf30c2f56` | `238bd54ccf2e60c974074ed55ed94b7d7892d862268409a04f347c18be3e597d` |
| Rust | `code-ir-factorial-pinned-rust-results-20260926/` | `eb8b41c271aa13160a1ba7ce63af7c8ea35461f3f1bfa8248a3845d19d44c98a` | `942c59e1e1f5258b411ead144019fb849525aed13472bf3e3391e8b6ea103c9e` | `b2933400f1b457807d2399810f29f3b43f37fa962fd6ebde6ab3ea2fc4a799d8` |
| TypeScript | `code-ir-factorial-pinned-typescript-results-20260926/` | `152a5562e1fbc0a44f8e7143fa29b9559c6a1ad4a5b85c309a092bb9814b1057` | `f3b03eaa8dc290792cb97f2e13eeb55d8c5ffd8a308603c0075bf080246b411d` | `ac4db853de18ef408fc6243622ada06ed02d3897bfa8b9f92f08661cb8c05ce4` |

Verification: `npm run build`, `npm run typecheck`, `npm run lint`,
`npm run format:check`, six Python offline runner tests, JS unit suite
(317 passed, one skipped), all four fresh five-arm evaluations, and separate
unchanged two-arm Go replay. The broad root/integration/e2e suites were not
rerun. No default index/ranker, application API or source fixtures changed.
