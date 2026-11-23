use crate::inventory::filters::{filters_from_signal, AuditFilters, ScanSignal};
use crate::inventory::registry::{Asset, AssetRegistry};
use crate::inventory::state::ScanState;

/// Wraps the registry scan and records every returned asset ID in the
/// shared scan state. The bare name deliberately collides with the
/// AssetRegistry::list_assets method.
pub fn list_assets(
    registry: &AssetRegistry,
    state: &mut ScanState,
    signal: &ScanSignal,
) -> Vec<Asset> {
    let filters: AuditFilters = filters_from_signal(signal);
    let assets = registry.list_assets(&filters);
    let ids = assets.iter().map(|asset| asset.id.clone()).collect::<Vec<_>>();
    record_scanned(state, &ids);
    assets
}

fn record_scanned(state: &mut ScanState, asset_ids: &[String]) {
    state.add(asset_ids);
}
