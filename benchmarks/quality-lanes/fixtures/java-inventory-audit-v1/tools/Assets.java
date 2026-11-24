package tools;

import audit.AuditLedger;
import audit.AuditRecord;
import audit.Review;
import inventory.Asset;
import inventory.AssetRegistry;
import inventory.ScanFilters;
import inventory.ScanState;
import java.util.List;
import java.util.Map;
import permissions.Enforcer;

/**
 * Tool-facing facade for the inventory scan and the permission-gated asset
 * selection. The static listAssets deliberately shares its bare name with
 * the inventory.AssetRegistry.listAssets method.
 */
public final class Assets {

    /** Asset IDs surfaced by one scan. */
    public record ScanResult(String recommendedId, List<String> alternativeIds, Map<String, String> parameters) {
    }

    /** Final summary returned to the harness after a selection. */
    public record AssetSummary(String assetId, String kind, String location, String parameters) {
    }

    public static List<Asset> listAssets(AssetRegistry registry, ScanState state, ScanFilters filters) {
        List<Asset> assets = registry.listAssets(filters);
        recordScanned(state, assets.stream().map(Asset::assetId).toList());
        return assets;
    }

    public static void recordScanned(ScanState state, List<String> assetIds) {
        if (state != null) {
            state.add(assetIds);
        }
    }

    static boolean isAssetInScanResult(ScanResult result, String assetId) {
        return assetId.equals(result.recommendedId())
                || result.alternativeIds().contains(assetId);
    }

    static void authorizeAccess(ScanResult result, String assetId, Enforcer enforcer) {
        if (!isAssetInScanResult(result, assetId)) {
            throw new IllegalArgumentException("asset not in current scan result");
        }
        enforcer.requirePermission("asset:select");
    }

    public static AssetSummary handleSelectAsset(
            Map<String, Object> context,
            AssetRegistry registry,
            ScanResult result,
            String assetId,
            Enforcer enforcer,
            AuditLedger ledger) {
        authorizeAccess(result, assetId, enforcer);
        if (!Review.checkAccess(context, ledger, assetId)) {
            throw new IllegalStateException("asset not covered by audit ledger");
        }
        Asset asset = registry.getAsset(assetId)
                .orElseThrow(() -> new IllegalArgumentException("unknown asset: " + assetId));
        ledger.append(new AuditRecord(assetId, "select", asset.location()));
        return applySelectedAsset(asset, result);
    }

    static AssetSummary applySelectedAsset(Asset asset, ScanResult result) {
        String parameters = result.parameters().getOrDefault(asset.assetId(), "");
        return new AssetSummary(asset.assetId(), asset.kind(), asset.location(), parameters);
    }
}
