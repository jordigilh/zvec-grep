export class AssetState {
  private readonly ids = new Set<string>();

  add(assetIds: readonly string[]): void {
    for (const assetId of assetIds) {
      if (assetId.length > 0) this.ids.add(assetId);
    }
  }

  contains(assetId: string): boolean {
    return this.ids.has(assetId);
  }
}

const assetStateKey = "assetState";

export function stateFromContext(context: Map<string, unknown>): AssetState {
  const state = context.get(assetStateKey);
  return state instanceof AssetState ? state : new AssetState();
}
