use std::collections::{HashMap, HashSet};

/// ScanState records asset IDs returned by list_assets for one selection
/// context. Rechecks and later scan calls share this object.
#[derive(Clone, Default)]
pub struct ScanState {
    ids: HashSet<String>,
}

impl ScanState {
    /// Records every non-empty asset ID returned by the scanner.
    pub fn add(&mut self, asset_ids: &[String]) {
        self.ids
            .extend(asset_ids.iter().filter(|id| !id.is_empty()).cloned());
    }

    pub fn contains(&self, asset_id: &str) -> bool {
        self.ids.contains(asset_id)
    }
}

pub fn scan_state_from_context(context: &HashMap<String, ScanState>) -> ScanState {
    context.get("scan_state").cloned().unwrap_or_default()
}
