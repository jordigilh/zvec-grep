package tools

import (
	"context"
	"errors"

	"example.com/synthetic-inventory-audit/internal/api"
	"example.com/synthetic-inventory-audit/internal/audit"
	"example.com/synthetic-inventory-audit/internal/inventory"
	"example.com/synthetic-inventory-audit/internal/permissions"
)

// ScanResult carries the asset IDs and parameters surfaced by one scan.
type ScanResult struct {
	RecommendedID  string
	AlternativeIDs []string
	Parameters     map[string]string
}

func isAssetInScanResult(result *ScanResult, assetID string) bool {
	if result == nil {
		return false
	}
	if result.RecommendedID == assetID {
		return true
	}
	for _, candidate := range result.AlternativeIDs {
		if candidate == assetID {
			return true
		}
	}
	return false
}

// authorizeAccess rejects asset IDs absent from the current scan result and
// enforces the asset:select permission before any registry lookup.
func authorizeAccess(result *ScanResult, assetID string, enforcer *permissions.Enforcer) error {
	if !isAssetInScanResult(result, assetID) {
		return errors.New("asset not in current scan result")
	}
	return enforcer.RequirePermission("asset:select")
}

// HandleSelectAsset guards the selection, resolves the registry entry,
// records the audit entry, and assembles the final summary.
func HandleSelectAsset(ctx context.Context, registry *inventory.AssetRegistry, result *ScanResult, assetID string, enforcer *permissions.Enforcer, ledger *audit.AuditLedger) (api.AssetSummary, error) {
	if err := authorizeAccess(result, assetID, enforcer); err != nil {
		return api.AssetSummary{}, err
	}
	if !audit.CheckAccess(ctx, ledger, assetID) {
		return api.AssetSummary{}, errors.New("asset not covered by audit ledger")
	}
	asset, ok := registry.GetAsset(assetID)
	if !ok {
		return api.AssetSummary{}, errors.New("unknown asset")
	}
	if err := ledger.Record(audit.AuditRecord{AssetID: assetID, Action: "select", Detail: asset.Location}); err != nil {
		return api.AssetSummary{}, err
	}
	return applySelectedAsset(asset, result), nil
}

func applySelectedAsset(asset inventory.Asset, result *ScanResult) api.AssetSummary {
	return api.AssetSummary{
		ID:         asset.ID,
		Kind:       asset.Kind,
		Location:   asset.Location,
		Parameters: result.Parameters[asset.ID],
	}
}
