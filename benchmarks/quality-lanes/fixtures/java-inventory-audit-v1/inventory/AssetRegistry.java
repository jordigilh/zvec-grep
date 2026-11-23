package inventory;

import java.util.List;
import java.util.Optional;

/**
 * Read-only registry of tracked assets. The scan path enumerates entries
 * through listAssets with converted filters; identity resolution goes
 * through getAsset before any selection is accepted.
 */
public final class AssetRegistry {
    private final List<Asset> assets;

    public AssetRegistry(List<Asset> assets) {
        this.assets = List.copyOf(assets);
    }

    /**
     * Lists registry assets that satisfy the scan filters.
     */
    public List<Asset> listAssets(ScanFilters filters) {
        return ScanFilters.applyTagFilters(assets, filters);
    }

    public Optional<Asset> getAsset(String assetId) {
        return assets.stream()
                .filter(asset -> asset.assetId().equals(assetId))
                .findFirst();
    }
}
