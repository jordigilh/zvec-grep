use crate::inventory::filters::{apply_tag_filters, AuditFilters};

/// Asset describes one inventory asset known to the registry.
#[derive(Clone)]
pub struct Asset {
    pub id: String,
    pub kind: String,
    pub location: String,
}

/// AssetRegistry is the authoritative listing of assets under audit.
pub struct AssetRegistry {
    assets: std::collections::HashMap<String, Asset>,
}

impl AssetRegistry {
    pub fn new(assets: Vec<Asset>) -> Self {
        Self {
            assets: assets
                .into_iter()
                .map(|asset| (asset.id.clone(), asset))
                .collect(),
        }
    }

    /// Lists registry entries matching the scan filters. The bare method
    /// name deliberately collides with the tools::list_assets wrapper.
    pub fn list_assets(&self, filters: &AuditFilters) -> Vec<Asset> {
        self.assets
            .values()
            .filter(|asset| apply_tag_filters(asset, filters))
            .cloned()
            .collect()
    }

    pub fn get_asset(&self, asset_id: &str) -> Option<&Asset> {
        self.assets.get(asset_id)
    }
}
