use std::collections::HashMap;

use crate::audit::ledger::{AuditLedger, AuditRecord};
use crate::audit::review::check_access;
use crate::audit::AssetSummary;
use crate::inventory::registry::{Asset, AssetRegistry};
use crate::inventory::state::ScanState;
use crate::permissions::enforcer::Enforcer;

/// ScanResult carries the asset IDs and parameters surfaced by one scan.
pub struct ScanResult {
    pub recommended_id: String,
    pub alternative_ids: Vec<String>,
    pub parameters: HashMap<String, String>,
}

fn is_asset_in_scan_result(result: &ScanResult, asset_id: &str) -> bool {
    if result.recommended_id == asset_id {
        return true;
    }
    result.alternative_ids.iter().any(|candidate| candidate == asset_id)
}

/// Rejects asset IDs absent from the current scan result and enforces the
/// asset:select permission before any registry lookup.
fn authorize_access(
    result: &ScanResult,
    asset_id: &str,
    enforcer: &Enforcer,
) -> Result<(), &'static str> {
    if !is_asset_in_scan_result(result, asset_id) {
        return Err("asset not in current scan result");
    }
    enforcer
        .require_permission("asset:select")
        .map_err(|_| "asset:select permission missing")
}

/// Guards the selection, resolves the registry entry, records the audit
/// entry, and assembles the final summary.
pub fn handle_select_asset(
    context: &HashMap<String, ScanState>,
    registry: &AssetRegistry,
    result: &ScanResult,
    asset_id: &str,
    enforcer: &Enforcer,
    ledger: &mut AuditLedger,
) -> Result<AssetSummary, &'static str> {
    authorize_access(result, asset_id, enforcer)?;
    if !check_access(context, ledger, asset_id) {
        return Err("asset not covered by audit ledger");
    }
    let asset = registry
        .get_asset(asset_id)
        .ok_or("unknown asset")?;
    ledger
        .record(AuditRecord {
            asset_id: asset_id.to_owned(),
            action: "select".to_owned(),
            detail: asset.location.clone(),
        })
        .map_err(|_| "audit:write permission missing")?;
    Ok(apply_selected_asset(asset, result))
}

fn apply_selected_asset(asset: &Asset, result: &ScanResult) -> AssetSummary {
    AssetSummary {
        id: asset.id.clone(),
        kind: asset.kind.clone(),
        location: asset.location.clone(),
        parameters: result
            .parameters
            .get(&asset.id)
            .cloned()
            .unwrap_or_default(),
    }
}
