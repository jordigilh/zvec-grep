import type { Asset } from "./registry.js";

export interface ScanSignal {
  tags: readonly string[];
}

export interface AuditFilters {
  requiredTags: readonly string[];
}

export function filtersFromSignal(signal: ScanSignal): AuditFilters {
  const requiredTags = signal.tags.filter((tag) => tag.length > 0);
  return { requiredTags };
}

export function applyTagFilters(asset: Asset, filters: AuditFilters): boolean {
  if (filters.requiredTags.length === 0) return true;
  // Asset kinds double as coarse tags for filtering purposes.
  return filters.requiredTags.includes(asset.kind);
}
