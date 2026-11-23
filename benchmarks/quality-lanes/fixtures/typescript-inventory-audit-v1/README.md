# Synthetic TypeScript Inventory Audit Fixture (HOLDOUT — never tuned on)

Holdout fixture for issue #68 (fusion recall trade): a never-tuned-on
TypeScript lane validating generalization of ranking work tuned on
`typescript-workflow-discovery-v1`. It is the TypeScript sibling of the Go
holdout `go-inventory-audit-v1`, porting that fixture's domain and query
archetypes into the TypeScript template fixture's schema and unit-naming
conventions. **No ranking arm has been run on this fixture; do not tune
against it.**

Domain: inventory audit with permission enforcement — an asset registry is
scanned, scanned-asset membership is tracked in shared state, scanner tags
become scan filters, permission policy gates ledger writes and selection,
audit review rechecks ledger coverage, and the selected asset's details are
copied into a final summary.

It is a small, source-controlled regression corpus for semantic code search:
production-shaped TypeScript symbols, realistic distractors, and eight query
intents covering exact lookup, policy enforcement, data flow, recheck state,
interactive guards, validation, failure modes, and presentation.

## Protocol

- **Source-authored truth**: the relevance judgments in `truth.json` were
  authored by the fixture's source author (the adjudication protocol used by
  the other synthetic lanes), not inferred from any backend or from agreement
  between engines.
- **Complete-universe qrels**: `qrels.json` was generated with
  `benchmarks/quality-lanes/tools/build_synthetic_qrels.py --fixture <this
  dir> --output <this dir>/qrels.json`, expanding the source-authored
  judgments to every manifest unit per query (`judgment_status=adjudicated`,
  `candidate_pool_complete=true`, `source_truth=true`; unlisted units get
  grade 0 with the standard negative rationale).
- Unit IDs are qualified source symbols such as
  `permissions/enforcer.ts::Enforcer.requirePermission` (path::ClassName,
  path::ClassName.method, path::functionName), matching the
  `typescript-workflow-discovery-v1` naming convention. Manifest line spans
  are exact: computed and verified programmatically (each unit's symbol
  identifier must appear within its span, method spans nest inside their
  class span, and no two sibling spans cross).

## Design rationale

- **Domain shift**: inventory audit with permission enforcement instead of
  workflow discovery. Same shape (scanner → shared membership state →
  tag-to-filter conversion → permission-gated selection → audit ledger →
  final summary) so the Go holdout's query archetypes transfer, with zero
  vocabulary overlap with the tuning fixture.
- **Same scale class**: 10 `.ts` files across `inventory/`, `permissions/`,
  `audit/`, `tools/`, `api/`; 43 manifest units; 8 queries — matching the
  TypeScript template fixture (8 files / 33 units / 8 queries) and the Go
  holdout (10 files / 45 units / 8 queries).

## Deliberate hard cases

- **Same-named exported function vs method** (the `ListWorkflows` collision
  class): `tools/list_assets.ts::listAssets` (exported function) calls
  `AssetRegistry.listAssets` (method). A unique-index-symbol scan must treat
  the bare name `listAssets` as ambiguous.
- **Same-named symbols in different modules**: `audit/review.ts::checkAccess`
  (exported function) and `permissions/enforcer.ts::Enforcer.checkAccess`
  (method) share the bare name `checkAccess`; `inventory`'s `Asset` type is
  referenced in `inventory/filters.ts` as well as its home module.
- **Type mentions in typed object literals** (the type-mention
  false-call-edge trap): `Permission` in `PolicySet.grant`,
  `ReviewFindings` in `findingsByAsset`, `AuditRecord` and `AssetSummary` in
  `handleSelectAsset`/`applySelectedAsset`.
- **Exported/unexported mix**: non-exported `assetStateKey` and
  `reviewBudget` consts and non-exported functions `assetInScanResult` and
  `authorizeAccess`, alongside exported interfaces, classes, methods,
  constructors, and functions.
- **Multi-hop recheck path**: `audit/review.ts::checkAccess` →
  `stateFromContext` → `AssetState.contains` plus `AuditLedger.entries`.
- **Empty-scan fail-closed path**: an empty scan leaves `AssetState` empty,
  which makes both the selection guard and the audit recheck fail closed.
- **`reviewBudget` is declared at the end of `audit/review.ts`** on purpose,
  mirroring the Go fixture's lesson: a braceless file-level const directly
  above a function declaration made line-span inference swallow that
  function. Trailing placement keeps the unexported-const unit without span
  overlap.

## Adjudication notes

Mirroring the Go holdout's deliberate grade-0 calls:

- `deny-unpermitted-asset` leaves `tools/select_asset.ts::authorizeAccess`
  at grade 0: that guard rejects *scan-result membership + asset:select
  permission* failures, not the *policy-never-granted* failure the query
  asks about.
- `forward-tags-to-scan` leaves `inventory/filters.ts::ScanSignal` at grade
  0: the query asks about the conversion path, not the raw signal shape.

The `deny-unpermitted-asset` / `enforce-permission-before-audit` judgment
overlap (`Enforcer.requirePermission`, `Enforcer.checkAccess`,
`AccessDenied` are relevant to both) is accepted knowingly: the two queries
ask about the same enforcement machinery from different entry points
(selection denial vs ledger write), mirroring the Go holdout.

## Regeneration

```
/opt/homebrew/bin/python3.12 benchmarks/quality-lanes/tools/build_synthetic_qrels.py \
  --fixture benchmarks/quality-lanes/fixtures/typescript-inventory-audit-v1 \
  --output benchmarks/quality-lanes/fixtures/typescript-inventory-audit-v1/qrels.json
```

## Status

Locked as a never-tuned-on holdout: pinned in
`baselines-multilanguage.json` (role: holdout) and gated in CI by
`check_baselines_multilanguage.py`. No tuned arm has been run on this
fixture.
