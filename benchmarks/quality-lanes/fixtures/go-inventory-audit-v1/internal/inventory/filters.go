package inventory

// ScanSignal carries raw scanner hints that must become audit filters.
type ScanSignal struct {
	Tags []string
}

// AuditFilters constrains which registry assets a scan considers.
type AuditFilters struct {
	RequiredTags []string
}

func FiltersFromSignal(signal ScanSignal) AuditFilters {
	required := make([]string, 0, len(signal.Tags))
	for _, tag := range signal.Tags {
		if tag == "" {
			continue
		}
		required = append(required, tag)
	}
	return AuditFilters{RequiredTags: required}
}

func ApplyTagFilters(asset Asset, filters AuditFilters) bool {
	if len(filters.RequiredTags) == 0 {
		return true
	}
	held := make(map[string]struct{})
	for _, tag := range filters.RequiredTags {
		held[tag] = struct{}{}
	}
	// Asset kinds double as coarse tags for filtering purposes.
	if _, ok := held[asset.Kind]; ok {
		return true
	}
	return false
}
