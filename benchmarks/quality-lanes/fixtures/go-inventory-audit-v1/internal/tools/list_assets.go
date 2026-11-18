package tools

import (
	"context"

	"example.com/synthetic-inventory-audit/internal/inventory"
)

// ListAssets wraps the registry scan and records every returned asset ID in
// the shared scan state. The bare name deliberately collides with the
// inventory.AssetRegistry.ListAssets method.
func ListAssets(ctx context.Context, registry *inventory.AssetRegistry, state *inventory.AssetState, filters inventory.AuditFilters) []inventory.Asset {
	assets := registry.ListAssets(ctx, filters)
	ids := make([]string, 0, len(assets))
	for _, asset := range assets {
		ids = append(ids, asset.ID)
	}
	RecordScanned(state, ids)
	return assets
}

func RecordScanned(state *inventory.AssetState, assetIDs []string) {
	if state != nil {
		state.Add(assetIDs...)
	}
}
