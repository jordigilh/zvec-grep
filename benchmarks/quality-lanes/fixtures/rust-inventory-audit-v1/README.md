# Rust Holdout Fixture: `rust-inventory-audit-v1`

Never-tuned-on holdout for issue #68 (fusion recall trade). This fixture was
authored after the fusion-recall work; no arm, weight, or parameter has been
evaluated or tuned against it. It exists to measure generalization only.

## Purpose

The tuning lane for Rust is `rust-workflow-discovery-v1` (workflow discovery
domain). This fixture is domain-shifted to **inventory audit with permission
enforcement** while keeping the same code shape (scanner lists assets →
shared scanned-ID membership state → tag-to-filter conversion →
permission-gated asset selection → audit ledger → final summary), so the
query archetypes transfer but vocabulary does not. It mirrors the design of
the Go holdout `go-inventory-audit-v1` and is schema-identical to the Rust
tuning fixture.

## Protocol

- **Source-authored truth**: the fixture author wrote both the sources and
  `truth.json` (8 queries, 4-5 judgments each, grades 0-3 with rationales
  grounded in this source code) — the adjudication protocol used by the
  other synthetic lanes.
- **Complete-universe qrels**: `qrels.json` was generated with
  `benchmarks/quality-lanes/tools/build_synthetic_qrels.py --fixture <this
  dir> --output <this dir>/qrels.json`, expanding the source-authored
  judgments to the complete 45-unit universe per query
  (`judgment_status=adjudicated`, `candidate_pool_complete=true`,
  `source_truth=true`).
- Manifest line spans were computed programmatically and verified: every
  unit's symbol text appears within its declared span (validation script
  kept outside the fixture; spans are exact).

## Design rationale

- **Same scale class as the tuning fixture**: 14 source files (one crate,
  `synthetic-inventory-audit`), 45 manifest units, 8 queries — versus
  12 files / 38 units / 8 queries for `rust-workflow-discovery-v1`.
- **Schema-faithful**: `manifest.json`, `truth.json`, `qrels.json`,
  `Cargo.toml`, `Cargo.lock`, `src/` mirror the Rust template fixture's
  field set and unit-id naming (`path::Struct`, `path::Struct.method`,
  `path::free_fn`, `path::module-declarations`).
- **Zero vocabulary overlap** with the tuning fixture's domain words.

## Hard cases deliberately included

1. **Same-named free function vs method** (the `list_workflows` collision
   class): `tools::list_assets` (free function) calls
   `AssetRegistry::list_assets` (method). A unique-index-symbol scan must
   treat the bare name as ambiguous.
2. **Type mentions in struct literals**: `Permission { ... }` in
   `PolicySet::grant`, `AuditRecord { ... }` in `handle_select_asset`,
   `ReviewFindings { ... }` in `findings_by_asset`, `AuditFilters { ... }`
   in `filters_from_signal`, `AssetSummary { ... }` in
   `apply_selected_asset`.
3. **Same-named symbols in different modules**: `audit::review::check_access`
   (free function) vs `permissions::Enforcer::check_access` (method); the
   `Asset` type is mentioned in `inventory::filters` and `tools` beyond its
   home module.
4. **Public/private mix**: private `record_scanned`,
   `is_asset_in_scan_result`, `authorize_access`, `apply_selected_asset`,
   `review_budget`; public functions, methods, constructors, and structs.
5. **Multi-hop recheck path**: `handle_select_asset` → `review::check_access`
   → `scan_state_from_context` → `ScanState::contains`.
6. **Empty-scan fail-closed path**: an empty `ScanState` makes both the
   selection guard and the audit recheck reject.

## Queries (8)

| id | intent |
|---|---|
| record-scanned-membership | exact implementation / data flow |
| deny-unpermitted-asset | policy check / semantic behavior |
| forward-tags-to-scan | data flow / semantic behavior |
| preserve-membership-on-recheck | multi-hop state / recheck flow |
| interactive-selection-guard | exact implementation / interactive guard |
| enforce-permission-before-audit | exact implementation / validation |
| empty-scan-fails-closed | behavior / failure mode |
| copy-selected-asset-details | multi-symbol data flow |

Deliberate grade-0 call: `forward-tags-to-scan` scores `ScanSignal` at 0
(raw tag carrier only). Judgment overlap between `deny-unpermitted-asset`
and `enforce-permission-before-audit` is accepted, mirroring the Go holdout.
