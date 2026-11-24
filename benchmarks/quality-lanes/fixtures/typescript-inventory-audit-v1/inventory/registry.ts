import type { AuditFilters } from "./filters.js";
import { applyTagFilters } from "./filters.js";

export interface Asset {
  assetId: string;
  kind: string;
  location: string;
}

export class AssetRegistry {
  constructor(private readonly assets: ReadonlyMap<string, Asset>) {}

  // listAssets deliberately shares its bare name with the tools/list_assets
  // wrapper function.
  listAssets(filters: AuditFilters): Asset[] {
    return [...this.assets.values()].filter((asset) => applyTagFilters(asset, filters));
  }

  getAsset(assetId: string): Asset | undefined {
    return this.assets.get(assetId);
  }
}
