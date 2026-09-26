# Code IR v2 frozen, syntax-only retrieval comparison (2026-09-26)

**Decision:** Keep v2 opt-in, with **no default ranking change**. Go and Python
fail the per-language aggregate no-regression gate. Rust and TypeScript pass
that *synthetic aggregate* gate, but have query-level losses; they also lack
independently judged real-repository retrieval cases. No lane is approved for
default enablement. SCIP, including any scip-go binary, is **not** used.

## Paired setup

This run uses the actual v2 projection published and read-validated from
[PR #15](https://github.com/jordigilh/zvec-grep/pull/15) via
`source-metadata-entity-v1`, not a second metadata/selection implementation.
Against it, `syntax-only` uses the existing code extractor and normal workspace
index. Both arms use this checkout's TypeScript hybrid search engine, the same
cached `local/potion-code-16m-v2` CPU model, zvec 0.7.0, query text, source
bytes and Engram's unchanged source-unit qrels/scorer, with top 10. All four
lanes have eight frozen queries. Each run uses separate freshly staged source
roots and indexes; only projection candidate text/unit selection differs. The
published IR snapshot keeps all semantic endpoints. Neither source fixture,
manifest, truth nor qrels was modified. Recomputed fixture/qrel source digests
and readback of the published projection must succeed before scoring.

These are **synthetic, source-authored adjudicated qrels**, not independently
judged real-Kubernaut relevance. Mapping raw ranked spans to qrel source units
uses Engram's unchanged `_unit_spans`, `_map_chunk` and `_dedupe_results`; it
is a line-overlap evaluation mapping, **not** an independently labeled
declaration byte-span conformance inventory. Qrel scoring rejects unjudged
units. See [plan and runner](./code-ir-v2-frozen-qeval-plan.md).

## Aggregate metrics

Values: **nDCG@10 / MRR@10 / Recall@10 / Precision@10**. Pass means *at least
one* aggregate metric improves and *none* regresses (tolerance 1e-12), **not**
authorization to turn on a lane.

| Language | Syntax-only control | Actual v2 projection | Aggregate gate |
|---|---|---|---|
| Go | 0.588774 / 0.733333 / 0.677083 / 0.250000 | **0.577130 / 0.733333 / 0.677083 / 0.250000** | Fail: nDCG −0.011644 |
| Python | 0.546521 / 0.547917 / 0.689583 / 0.250000 | **0.537513 / 0.530357 / 0.689583 / 0.250000** | Fail: nDCG −0.009007, MRR −0.017560 |
| Rust | 0.416330 / 0.455556 / 0.560417 / 0.200000 | **0.459189 / 0.455853 / 0.647917 / 0.237500** | Pass aggregate, two query losses |
| TypeScript | 0.426791 / 0.591667 / 0.550000 / 0.200000 | **0.465124 / 0.593056 / 0.633333 / 0.225000** | Pass aggregate, one query loss |

These controls match the previously recorded syntax-only baseline scores for
all four languages. Direct v2 text is *not identical* to the earlier
second-extractor experimental metadata arm: the in-frontend Go documentation
coverage, signature formatting and v2 bounded text differ. Hence Go's v2
nDCG (0.577130) differs from the old source-parity experiment (0.586157).
No older experiment is substituted for this run.

## All eight query-level deltas

`ΔN/M/R/P` means v2 minus syntax-only nDCG/MRR/recall/precision@10. Rounded
to three decimals here; full-precision values and raw ranks are retained in
the external artifacts. A negative value is a query loss even if its lane
passes the aggregate gate.

| Query | Go ΔN/M/R/P | Python ΔN/M/R/P | Rust ΔN/M/R/P | TypeScript ΔN/M/R/P |
|---|---|---|---|---|
| record-discovered-membership | +0.069 / +0.000 / +0.250 / +0.100 | +0.000 / +0.000 / +0.000 / +0.000 | +0.305 / +0.000 / +0.500 / +0.200 | +0.000 / +0.000 / +0.000 / +0.000 |
| reject-undiscovered-workflow | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.022 / +0.043 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| forward-labels-to-discovery | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 |
| preserve-membership-on-retry | −0.003 / +0.000 / +0.000 / +0.000 | −0.036 / −0.057 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |
| interactive-selection-guard | −0.125 / +0.000 / −0.250 / −0.100 | +0.000 / +0.000 / +0.000 / +0.000 | −0.009 / +0.000 / +0.000 / +0.000 | +0.012 / +0.000 / +0.000 / +0.000 |
| validate-workflow-parameters | +0.000 / +0.000 / +0.000 / +0.000 | −0.036 / −0.083 / +0.000 / +0.000 | −0.045 / −0.083 / +0.000 / +0.000 | +0.298 / +0.000 / +0.667 / +0.200 |
| empty-discovery-fails-closed | −0.033 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.071 / +0.043 / +0.200 / +0.100 | +0.005 / +0.011 / +0.000 / +0.000 |
| copy-selected-workflow-details | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 | +0.000 / +0.000 / +0.000 / +0.000 |

**Loss inspection:** Go `interactive-selection-guard` loses the relevant
`DiscoveryResult` from @10 (control rank 9, v2 absent), while its
`HandleSelectWorkflow` and `isWorkflowInDiscoveryResult` move from ranks 2/3
to 3/4. Python's `preserve-membership-on-retry` moves relevant hits from 5/8
to 7/10; `validate-workflow-parameters` shifts relevant hits from 3/6 to 4/7.
Rust's `handle_select_workflow` drops from rank 4 to 5 for
`interactive-selection-guard`; its `validate_workflow_parameters` from 3 to 4.
TypeScript's `WorkflowCatalog.listActions` drops from 5 to 6 for
`forward-labels-to-discovery`. These are diagnostic observations, **not**
query-specific ranking adjustments.

## Provenance, artifacts and reproduction

Run the committed code at `5c45808` (child of PR #15) after `npm run build`.
For each language, substitute the fixture name and **new**, distinct output
paths; this example is Go (no scip-go binary or SCIP index):

```sh
python3 scripts/replay_code_ir_v2_qeval.py \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /path/to/fresh-go-work \
  --output-dir /path/to/fresh-go-results
```

The run refuses existing work/output directories and refuses to place either
inside the fork or Engram checkouts. Each result directory has
`raw-runs.json`, `normalized-runs.json`, `metrics-k10.json`, `comparison.json`,
`run-manifest.json`. The manifest pins fixture, model cache, unchanged scorer,
mapping script, engine dist, adapter, query counts and output hashes. The
shared model-cache tree SHA-256 is
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`.
The final artifacts on this host are under
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`:

| Language | Final result directory | Frozen source SHA-256 | `metrics-k10.json` SHA-256 |
|---|---|---|---|
| Go | `code-ir-v2-final-go-results-20260926/` | `94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f` | `916cd4261512934f262a85c6122251d58baf05ab1bac670230d1ec45ab19c741` |
| Python | `code-ir-v2-final-python-results-20260926/` | `0b77b06131e0a0e8ffbafa258c54e8900b7c32c2ea6f156ea75e8c6a181e076b` | `78d5d910c348a7c6ff48c17aaba27ac88654e094c1e2cf13c31bf0e66dac96ea` |
| Rust | `code-ir-v2-final-rust-results-20260926/` | `eb8b41c271aa13160a1ba7ce63af7c8ea35461f3f1bfa8248a3845d19d44c98a` | `45ea38f83262665a78c78451eea035f09e50953725b5090a18da81e4a38866c2` |
| TypeScript | `code-ir-v2-final-typescript-results-20260926/` | `152a5562e1fbc0a44f8e7143fa29b9559c6a1ad4a5b85c309a092bb9814b1057` | `c51148d5f8bd6644b6d34691f3fdec3066b15acb2651f67a11e8bedcabd05c8f` |

All four runs used the same model cache hash and committed runner/runtime
revision. The policy selected 38/68 Go, 33/53 Python, 49/63 Rust and 38/46
TypeScript IR units for standalone ranked records; *all* IR units and facts
remain in their snapshots. The source checksum and mapping scorer remain
independent of the new projection. The earlier broad root suite and real-
repository retrieval judgments are not verified by this run. Next: review
query-level failures and independent real-repository judgments before any
lane-specific enablement proposal; Go/Python remain failed on aggregate.

Verification for this review slice: `npm run build`, `npm run lint`,
`npm run format:check`, `npm run typecheck`, four Python offline runner tests,
four actual paired qeval runs, and the JS unit suite (313 passed, 1 skipped).
The Engram frozen fixture tree remained unchanged. Root/integration/e2e suites
were not rerun.
