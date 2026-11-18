# Go Holdout Fixture: `go-inventory-audit-v1` — LOCKED (user lock, 2026-11-16)

Vendored copy for the zvec-grep quality lanes; the original report's title
read "DRAFT (not locked)" and was stale — the fixture was user-locked on
2026-11-16 after adversarial review (fixes applied and re-verified). The
body below is unchanged from the original build report; see the lane README
(`benchmarks/quality-lanes/README.md`) for the baseline reconciliation note
(the REPORT baseline below predates the promotion/fusion retune and used a
different harness; the Step 0 numbers in `baselines.json` are the lane
reference).

Built 2026-11-16 to validate generalization of the combined rerank arm
(`0.80*hybrid + 0.10*name_overlap + 0.05*token_sketch + 0.05*graph_signal`,
Go-gated; measured +0.045 nDCG / +0.146 MRR / 0 recall on
`go-workflow-discovery-v1`). The arm has **not** been run on this fixture.
This directory holds the fixture, compiler truth, and baseline measurements
only. The arm run happens after the parallel session lands and this fixture
is reviewed and locked.

## Design rationale

- **Domain shift**: inventory audit with permission enforcement, instead of
  workflow discovery. Same shape (scanner → shared membership state →
  policy-gated selection → ledger → final summary) so the query archetypes
  transfer, but zero vocabulary overlap with the tuning fixture.
- **Same scale class**: 10 Go files, one module
  (`example.com/synthetic-inventory-audit`), 45 manifest units, 8 queries —
  matching `go-workflow-discovery-v1` (10 files / 38 units / 8 queries).
- **Schema-faithful**: `manifest.json`, `qrels.json`, `truth.json`,
  `README.md`, `go.mod`, `internal/` mirror the existing fixture
  field-for-field. Qrels built with the engram repo's
  `scripts/build_synthetic_qrels.py` (source-authored truth expanded to the
  complete 45-unit universe per query; `judgment_status=adjudicated`,
  `candidate_pool_complete=true`, `source_truth=true`). Protocol:
  `synthetic-workflow-discovery-v1/source-unit@10`, raw limit 10,
  source-unit normalization.
- **Headroom, not ceiling**: the untouched-214ba02 baseline retrieves only
  30 of 45 units across all top-10s and scores 0.0 on the multi-hop recheck
  query — a genuine generalization test rather than a saturated one.

## Hard cases deliberately included

1. **Receiver-qualified method call with same-named bare function** (the
   `ListWorkflows` collision class): `tools.ListAssets` (bare function)
   calls `inventory.AssetRegistry.ListAssets` (method). Compiler truth
   contains the edge `ListAssets → AssetRegistry.ListAssets`; a
   unique-index-symbol scan must drop it as ambiguous.
2. **Type declarations referenced in composite literals** (type-mention
   false-call-edge trap): `Permission{...}` in `PolicySet.Grant`,
   `AuditRecord{...}` and `AssetSummary{...}` in `HandleSelectAsset`,
   `inventory.Asset{...}` and `ReviewFindings{...}` in `internal/audit`,
   `AssetState{...}`/`AuditFilters{...}`/`PolicySet{...}` constructors.
3. **Cross-package same-named symbols**: `audit.CheckAccess` (bare
   function) vs `permissions.Enforcer.CheckAccess` (method) share the bare
   name across packages; `inventory.Asset` is type-mentioned in
   `internal/tools` as well as its home package.
4. **Exported/unexported mix**: unexported `assetStateKey`, `reviewBudget`,
   `isAssetInScanResult`; exported functions, methods, constructors,
   types, and an error type with an `Error` method.

## Queries (8) and qrels

| id | intent | grade-2+ units |
|---|---|---:|
| record-scanned-membership | exact implementation / data flow | 4 |
| deny-unpermitted-asset | policy check / semantic behavior | 4 |
| forward-tags-to-scan | data flow / semantic behavior | 4 |
| preserve-membership-on-recheck | multi-hop state / recheck flow | 4 |
| interactive-selection-guard | exact implementation / interactive guard | 4 |
| enforce-permission-before-audit | exact implementation / validation | 4 |
| empty-scan-fails-closed | behavior / failure mode | 5 |
| copy-selected-asset-details | multi-symbol data flow | 4 |

## Compiler truth

Tool: copy of `go-graph-truth-output/tool` (go/packages + SSA + RTA) with
only fixture-dir/module-prefix/output-path parameterized via
`GRAPHTRUTH_*` env vars; analysis logic unchanged.

- **Root policy** (same as the tuning fixture): the module has no `main`
  package and no `_test.go` files, so RTA is rooted at **all 29
  functions/methods declared in the module's own packages** (exported and
  unexported) — the complete static call relation over fixture callables.
- **Result: 29 roots, 17 call edges** (all static; no interface dispatch).
  Every edge endpoint maps to a manifest unit. The edge
  `ListAssets → AssetRegistry.ListAssets` is the receiver-clash edge a
  name-mention scan must miss (hard case 1). Edge set is unchanged by the
  post-review const move (verified pair-for-pair).
- Artifacts: `truth/truth.json` (schema `go-graph-truth-v1`),
  `truth/work/compiler-edges.json` (raw tool output),
  `truth/tool/` (parameterized tool copy).

## Baseline measurements (untouched engine at 214ba02)

Detached git worktree of `jordigilh/zvec-grep-fork` at commit
`214ba026c2d05d9ebc1abed279b3adb22f6ce607` (`fix: keep projection embeddings
within input budget`), `npm ci` + `npm run build`, run with a copy of the
`go-lane-combined-rerank-output/harness/run-lane.mjs` driver (baseline-only
path: `--arms baseline`, no sidecar), model
`local/potion-code-16m-v2` from the shared model cache, 8 queries, raw
limit 10, 2 repetitions. Scored with the frozen-pool
`evaluate_semantic_search.py` via a copy of the harness `evaluate.py`
(source-unit normalization). The worktree was removed after the runs; the
shared checkout, its branches, and its pre-existing dirty state were not
touched.

| rep | nDCG@10 | MRR@10 | Recall@10 | Precision@10 |
|---|---|---|---|---|
| r1 | 0.5946 | 0.7875 | 0.725 | 0.275 |
| r2 | 0.5946 | 0.7875 | 0.725 | 0.275 |

Reps are identical (same ranked paths/lines/scores/matchedBy) — the engine
is deterministic on this fixture. For context only: the TypeScript lexical
champion on the tuning fixture was nDCG 0.589 / MRR 0.733 / recall 0.677 /
precision 0.250; this holdout baseline lands in the same class.

Per-query highlights (r1 = r2): `preserve-membership-on-recheck`
(Q4) scores **0.173 nDCG / 0.1 MRR / 0.25 recall** — nonzero but low,
i.e. real headroom honestly measured now that `audit.CheckAccess` is
credited when retrieved (pre-fix it was mis-scored 0.0 because the
braceless `reviewBudget` const span swallowed CheckAccess's declaration);
`empty-scan-fails-closed` (Q7) recall is **0.8** (was 0.6 pre-fix for the
same reason); `copy-selected-asset-details` 0.941 nDCG;
`deny-unpermitted-asset` MRR 0.2. 29 of 45 units ever retrieved
(pre-fix run: 30) — retrieval profile preserved.

## Fix log (adversarial review: LOCK AFTER FIXES — applied 2026-11-16)

1. **BLOCKING — `reviewBudget` span overlap**: moved the braceless
   file-level `const reviewBudget = 100` from directly above
   `audit.CheckAccess` (review.go:10) to the end of review.go, after
   `FindingsByAsset` (chosen option (a); the unexported-const manifest unit
   survives, and the exported/unexported mix keeps
   `assetStateKey`/`isAssetInScanResult` regardless). Verified with the
   frozen `_unit_spans`: `CheckAccess` now spans 13–22 and `reviewBudget`
   64–64, no overlap. Did NOT add explicit start_line/end_line (schema
   deviation). No call edges changed (compiler truth edge set identical
   pair-for-pair).
2. **Cosmetic — false documentation claim**: README and REPORT no longer
   claim `inventory.Asset` is type-mentioned in `internal/audit`
   (review.go mentions only `AssetStateFromContext`/`AssetState.Contains`;
   the `Asset` type-mention sites are in `internal/tools` and its home
   package).
3. **Cosmetic — adjudication note**: README now documents the two
   deliberate grade-0 calls (Q2 `authorizeAccess`=0, Q3 `ScanSignal`=0)
   and the knowingly-accepted Q2/Q6 judgment overlap.
4. **Regenerated**: qrels.json (unit set unchanged at 45; snapshot digest
   changed), compiler truth (unchanged 17 edges; truth.json re-emitted with
   a new file hash), baseline runs r1/r2, and all hashes below.

## Reproducibility

- Fixture source: 10 files, 10515 bytes, snapshot_sha256
  `87bd1431d3ee7a64dd8ed5476587d7ddc1606af65c02be7c2196b0a5ed0b3c98`
- `fixture/manifest.json` sha256
  `9ff474190cc1756c66f2e693f0596a94a195b03cc53c554383c5489c8f195d22`
- `fixture/qrels.json` sha256
  `793cc291d86a912300ce9109e0673f28507af3637e0d6c3f03a08387bf76706c`
- `fixture/truth.json` sha256
  `360bb826fa4a1b2b4a0469e2a5c460d8e8487282e4b829b85ee89786cf4dfb79`
- `truth/truth.json` (compiler truth) sha256
  `f32935fa50aa4ce506b09289c5c904a94e4635d285cedc02e0be27910d01903a`
- Engine: zvec-grep-fork @ `214ba026c2d05d9ebc1abed279b3adb22f6ce607`,
  detached worktree (removed after runs; recreate with
  `git worktree add --detach <dir> 214ba02`, `npm ci`, `npm run build`).
- Model cache: shared `go-lane-combined-rerank-output/model-cache`.
- Captures embed `manifest_sha256` / `qrels_sha256` matching the hashes
  above and the fixture id `go-inventory-audit-v1`.

## Artifacts

- `fixture/` — the new fixture (DRAFT): manifest.json (45 units),
  truth.json (8 source-authored queries), qrels.json (adjudicated,
  complete pool), README.md, go.mod, internal/ (10 .go files).
- `truth/` — compiler truth: truth.json (17 edges, root policy),
  work/compiler-edges.json, tool/ (parameterized copy).
- `baseline/` — runs/r1/, runs/r2/ (capture.json + report.json each),
  work/ (staged fixture copies), harness/ (run-lane.mjs + evaluate.py
  copies; originals untouched).
- `REPORT.md` — this file.
