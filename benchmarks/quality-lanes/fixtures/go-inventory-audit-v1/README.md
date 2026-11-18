# Synthetic Go Inventory Audit Fixture (HOLDOUT — DRAFT)

Holdout fixture for validating generalization of ranking arms tuned on
`go-workflow-discovery-v1`. Domain: inventory audit with permission
enforcement — an inventory registry is scanned, scanned-asset membership is
tracked in shared state, permission policy gates ledger writes and
selection, and audit review rechecks trust ledger coverage.

This fixture is a small, source-controlled regression corpus for semantic
code search. It contains production-shaped Go symbols, realistic distractors,
and eight query intents covering exact lookup, policy enforcement, data
flow, recheck state, interactive guards, validation, failure modes, and
presentation.

The relevance truth in `truth.json` is authored from the fixture source. It
is not inferred from CocoIndex, zvec-git, Sense, or agreement between
backends. Run `scripts/build_synthetic_qrels.py` (engram repo) to expand the
source-authored targets to the complete unit universe and produce
`qrels.json`.

The unit IDs are qualified source symbols such as
`internal/permissions/enforcer.go::Enforcer.RequirePermission`. This makes
the fixture stable across chunk sizes and result formatting while keeping
the source anchor auditable.

## Deliberate hard cases

- **Receiver-qualified method call with a same-named bare function** (the
  `ListWorkflows` collision class): `tools.ListAssets` calls
  `inventory.AssetRegistry.ListAssets`; the bare method name `ListAssets`
  is shared by the tools function and the registry method, so
  unique-index-symbol resolution must drop those edges as ambiguous.
- **Cross-package same-named symbols**: `audit.CheckAccess` (bare function)
  and `permissions.Enforcer.CheckAccess` (method) share the bare name
  `CheckAccess` across packages; `inventory.Asset` is mentioned as a type
  in `internal/tools` as well as its home package.
- **Type declarations referenced in composite literals** (the type-mention
  trap that produced false call edges): `Permission{...}` in
  `PolicySet.Grant`, `AuditRecord{...}` in `HandleSelectAsset`,
  `AssetSummary{...}` in `HandleSelectAsset`, `ReviewFindings{...}` in
  `internal/audit/review.go`,
  `AssetState{...}`/`AuditFilters{...}` constructors.
- **Exported/unexported mix**: unexported `assetStateKey`, `reviewBudget`,
  `isAssetInScanResult`; unexported struct fields; exported methods,
  functions, constructors, and error type.
- **`reviewBudget` is declared at the end of review.go** on purpose: a
  braceless file-level const directly above a func declaration makes the
  frozen line-span inference swallow that func's declaration (adversarial
  review finding). Trailing placement keeps the unexported-const unit
  without span overlap.

## Adjudication notes (post-review)

Two deliberate grade-0 calls, kept as family-consistent with
`go-workflow-discovery-v1`:

- Q2 (`deny-unpermitted-asset`) leaves `tools.authorizeAccess` at grade 0:
  that guard rejects *scan-result membership + asset:select permission*
  failures, not the *policy-never-granted* failure the query asks about.
  The tuning fixture's analogous query likewise graded only the
  validator-side units.
- Q3 (`forward-tags-to-scan`) leaves `inventory.ScanSignal` at grade 0:
  the query asks about the conversion path, not the raw signal shape; the
  tuning fixture's analogous query graded only `DiscoveryFilters` (grade 1)
  among filter types.

Q2/Q6 judgment overlap (`Enforcer.RequirePermission`,
`Enforcer.CheckAccess`, `AccessDenied` are relevant to both) is accepted
knowingly: the two queries ask about the same enforcement machinery from
different entry points (selection denial vs ledger write), mirroring the
tuning fixture's Q2/Q6 overlap on `Validator.Validate`/`IsAllowed`.

## Status

DRAFT for user review. Do not treat as locked. The tuned arm has NOT been
run on this fixture.
