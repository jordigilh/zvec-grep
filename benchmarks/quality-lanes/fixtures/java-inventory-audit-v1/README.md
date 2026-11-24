# Synthetic Java Inventory Audit Fixture (HOLDOUT — never tuned on)

Holdout fixture for validating generalization of ranking arms tuned on
`java-workflow-discovery-v1` (issue #68, fusion recall trade). Domain:
inventory audit with permission enforcement — an asset registry is
scanned, scanned-asset membership is tracked in shared state, signal tags
are converted into scan filters, permission policy gates asset selection
and ledger appends, and audit review rechecks ledger coverage before a
final summary is assembled.

This fixture has **never been tuned on**: no ranking arm, rerank weight,
or retrieval parameter has been adjusted against it, and none of its
queries or units were used in any tuning run. It exists purely to measure
generalization of whatever was tuned on the sibling Java tuning fixture.

It is a small, source-controlled regression corpus for semantic code
search: production-shaped Java symbols, realistic distractors, and eight
query intents covering exact lookup, policy enforcement, data flow,
recheck state, interactive guards, validation, failure modes, and
presentation.

## Layout

| Path | Units | Role |
| --- | --- | --- |
| `inventory/Asset.java` | 1 | Immutable asset record |
| `inventory/AssetRegistry.java` | 4 | Registry listing with scan filters, identity lookup |
| `inventory/ScanState.java` | 4 | Shared scanned-ID set and context recovery |
| `inventory/ScanFilters.java` | 5 | Signal-tag to filter conversion, tag filtering, term normalizer |
| `permissions/PolicySet.java` | 5 | Grant set and permission record |
| `permissions/Enforcer.java` | 5 | Fail-closed permission gate and AccessDenied error |
| `audit/AuditRecord.java` | 1 | One ledger entry |
| `audit/AuditLedger.java` | 4 | Permission-gated append-only ledger |
| `audit/Review.java` | 6 | Recheck over shared state, findings, severity summary |
| `tools/Assets.java` | 9 | Scan/selection facade, guard, final summary assembly |

10 `.java` files, 340 source lines, 44 manifest units, 8 queries.
One public top-level type per file, Java convention.

## Protocol

- **Source-authored truth**: `truth.json` grades were written by the
  author of the synthetic sources against the source code itself
  (adjudication protocol). They are not inferred from any search backend.
- **Complete-universe qrels**: `qrels.json` was produced with
  `benchmarks/quality-lanes/tools/build_synthetic_qrels.py`, expanding
  the source-authored judgments to a judgment for every one of the 44
  manifest units in every query (8 x 44 = 352 rows, grade 0 default for
  units the truth did not grade). `judgment_status` is
  `"adjudicated"` **by construction**: the adjudicator is the source
  author, so no separate blind-audit reconciliation step applies.
- **Never tuned on**: see above.

To regenerate `qrels.json`:

```sh
/opt/homebrew/bin/python3.12 benchmarks/quality-lanes/tools/build_synthetic_qrels.py \
  --fixture benchmarks/quality-lanes/fixtures/java-inventory-audit-v1 \
  --output benchmarks/quality-lanes/fixtures/java-inventory-audit-v1/qrels.json
```

`manifest.json` line spans were computed programmatically from the
sources and verified: every unit's `symbol` text appears within its
`start_line`..`end_line` span, and no two units on the same file share an
identical span.

## Design rationale

Mirrors the Go holdout `go-inventory-audit-v1` (domain, query
archetypes, hard cases) while staying schema-identical to the Java
tuning fixture `java-workflow-discovery-v1` (manifest fields, unit_id
naming `path::Type` / `path::Type.member`, constructors as
`Type.constructor`, nested records as `Outer.Inner`, span + anchor
fields). Same shape as the tuning lane — scanner lists assets → shared
scanned-ID membership state → tag-to-filter conversion →
permission-gated asset selection → audit ledger → final summary — so
the query archetypes transfer, but the domain vocabulary is disjoint
from the tuning fixture.

## Deliberate hard cases

1. **Same-named static vs instance method** (the `ListAssets` collision
   class): `tools.Assets.listAssets` (static) calls
   `inventory.AssetRegistry.listAssets` (instance method). A
   unique-index-symbol lookup must disambiguate by qualifier.
2. **Cross-package same-named symbols**: `audit.Review.checkAccess`
   (static method) and `permissions.Enforcer.checkAccess` (instance
   method) share the bare name `checkAccess` across packages;
   `inventory.Asset` is type-mentioned in `tools/Assets.java` as well as
   its home package.
3. **Public/package-private mix**: package-private `Normalize` class and
   its package-private `term` method; package-private
   `Assets.isAssetInScanResult`, `Assets.authorizeAccess`, and
   `Assets.applySelectedAsset`; public classes, records, methods,
   constructors, and a nested public error type.
4. **Type mentions in constructor calls** (type-mention false-call-edge
   trap): `new ScanFilters(...)` in `fromSignal`, `new Permission(...)`
   parameter in `PolicySet.grant`, `new AuditRecord(...)` in
   `handleSelectAsset`, `new AssetSummary(...)` in `applySelectedAsset`,
   `new ReviewFindings(...)` in `findingsByAsset`.
5. **Multi-hop recheck path**: `handleSelectAsset` →
   `Review.checkAccess` → `ScanState.stateFromContext` →
   `ScanState.contains` (+ `AuditLedger.entries`), reusing shared state
   across rechecks.
6. **Empty-scan fail-closed path**: an empty `ScanState` (nothing added
   by `recordScanned`) makes `contains` false for every ID, failing both
   `Review.checkAccess` and `authorizeAccess` closed.

## Queries (8)

| id | intent | graded units |
|---|---|---:|
| record-scanned-membership | exact implementation / data flow | 5 |
| deny-unpermitted-asset | policy check / semantic behavior | 4 |
| forward-tags-to-scan | data flow / semantic behavior | 4 |
| preserve-membership-on-recheck | multi-hop state / recheck flow | 4 |
| interactive-selection-guard | exact implementation / interactive guard | 4 |
| enforce-permission-before-audit | exact implementation / validation | 4 |
| empty-scan-fails-closed | behavior / failure mode | 5 |
| copy-selected-asset-details | multi-symbol data flow | 4 |

All grades are 1-3 in `truth.json`; grade-0 negatives are added only by
the complete-universe qrels expansion. Q2/Q6 judgment overlap
(`Enforcer.requirePermission`, `Enforcer.checkAccess`, `AccessDenied`
are relevant to both) is accepted knowingly: the two queries ask about
the same enforcement machinery from different entry points (selection
denial vs ledger append), mirroring the Go holdout's Q2/Q6 overlap.

## Provenance

- Source: synthetic, authored for this fixture. No upstream repository,
  commit, or third-party code is involved.
- Repository: `synthetic/java-inventory-audit`, revision `fixture-v1`.
- Only the Java standard library is used; no build tool or external
  dependency is required.
- Snapshot digest in `manifest.json` / `qrels.json` follows the
  `build_synthetic_qrels.py` recipe (sorted paths, `path \0 bytes \0`).

## Status

HOLDOUT for issue #68 (fusion recall trade). Never tuned on. No
baseline or tuned-arm runs are stored in this directory.
