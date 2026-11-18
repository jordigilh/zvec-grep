package inventory

import (
	"context"
)

// Asset describes one inventory asset known to the registry.
type Asset struct {
	ID       string
	Kind     string
	Location string
}

// AssetRegistry is the authoritative catalog of assets under audit.
type AssetRegistry struct {
	assets map[string]Asset
}

func NewAssetRegistry(assets map[string]Asset) *AssetRegistry {
	if assets == nil {
		assets = make(map[string]Asset)
	}
	return &AssetRegistry{assets: assets}
}

// ListAssets returns registry entries matching the scan filters. The bare
// method name deliberately collides with the tools.ListAssets wrapper.
func (r *AssetRegistry) ListAssets(ctx context.Context, filters AuditFilters) []Asset {
	candidates := make([]Asset, 0, len(r.assets))
	for _, asset := range r.assets {
		if ApplyTagFilters(asset, filters) {
			candidates = append(candidates, asset)
		}
	}
	return candidates
}

func (r *AssetRegistry) GetAsset(assetID string) (Asset, bool) {
	asset, ok := r.assets[assetID]
	return asset, ok
}
