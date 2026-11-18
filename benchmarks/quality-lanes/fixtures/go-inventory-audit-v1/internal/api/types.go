package api

// AssetSummary is the presentation shape returned for a selected asset.
type AssetSummary struct {
	ID         string
	Kind       string
	Location   string
	Parameters string
}
