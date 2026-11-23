pub mod ledger;
pub mod review;

/// AssetSummary is the presentation shape returned for a selected asset.
pub struct AssetSummary {
    pub id: String,
    pub kind: String,
    pub location: String,
    pub parameters: String,
}
