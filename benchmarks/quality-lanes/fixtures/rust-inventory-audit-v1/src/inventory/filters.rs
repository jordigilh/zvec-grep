use crate::inventory::registry::Asset;

/// ScanSignal carries raw scanner hints that must become audit filters.
pub struct ScanSignal {
    pub tags: Vec<String>,
}

/// AuditFilters constrains which registry assets a scan considers.
pub struct AuditFilters {
    pub required_tags: Vec<String>,
}

pub fn filters_from_signal(signal: &ScanSignal) -> AuditFilters {
    AuditFilters {
        required_tags: signal
            .tags
            .iter()
            .filter(|tag| !tag.is_empty())
            .cloned()
            .collect(),
    }
}

pub fn apply_tag_filters(asset: &Asset, filters: &AuditFilters) -> bool {
    if filters.required_tags.is_empty() {
        return true;
    }
    // Asset kinds double as coarse tags for filtering purposes.
    filters.required_tags.iter().any(|tag| tag == &asset.kind)
}
