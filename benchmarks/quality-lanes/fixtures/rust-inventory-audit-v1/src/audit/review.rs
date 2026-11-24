use std::collections::HashMap;

use crate::audit::ledger::{AuditLedger, AuditRecord};
use crate::inventory::state::{scan_state_from_context, ScanState};

/// Verifies that an asset was part of the current scan and that the ledger
/// already covers it, before review findings are trusted. The bare name
/// deliberately collides with the permissions::Enforcer::check_access method.
pub fn check_access(
    context: &HashMap<String, ScanState>,
    ledger: &AuditLedger,
    asset_id: &str,
) -> bool {
    let state = scan_state_from_context(context);
    if !state.contains(asset_id) {
        return false;
    }
    !ledger.entries(asset_id).is_empty()
}

/// ReviewFindings summarizes the audit outcome for one asset.
pub struct ReviewFindings {
    pub asset_id: String,
    pub severity: String,
    pub notes: Vec<String>,
}

pub fn findings_by_asset(records: &[AuditRecord]) -> HashMap<String, Vec<ReviewFindings>> {
    let mut by_asset: HashMap<String, Vec<ReviewFindings>> = HashMap::new();
    for record in records.iter().take(review_budget()) {
        let severity = if record.action == "remediate" {
            "warning"
        } else {
            "info"
        };
        by_asset
            .entry(record.asset_id.clone())
            .or_default()
            .push(ReviewFindings {
                asset_id: record.asset_id.clone(),
                severity: severity.to_owned(),
                notes: vec![record.detail.clone()],
            });
    }
    by_asset
}

// review_budget caps how many findings one review pass considers.
fn review_budget() -> usize {
    100
}
