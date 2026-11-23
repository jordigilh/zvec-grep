import type { AuditFilters } from "../inventory/filters.js";
import type { Asset, AssetRegistry } from "../inventory/registry.js";
import type { AssetState } from "../inventory/state.js";

// listAssets wraps the registry scan and records every returned asset ID in
// the shared scan state. The bare name deliberately collides with the
// AssetRegistry.listAssets method.
export function listAssets(
  registry: AssetRegistry,
  state: AssetState,
  filters: AuditFilters,
): Asset[] {
  const assets = registry.listAssets(filters);
  recordScanned(state, assets.map((asset) => asset.assetId));
  return assets;
}

export function recordScanned(state: AssetState, assetIds: readonly string[]): void {
  state.add(assetIds);
}
