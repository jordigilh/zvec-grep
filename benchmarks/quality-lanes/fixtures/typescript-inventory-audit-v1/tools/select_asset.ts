import type { AuditLedger, AuditRecord } from "../audit/ledger.js";
import { checkAccess } from "../audit/review.js";
import type { AssetSummary } from "../api/summary.js";
import type { Asset, AssetRegistry } from "../inventory/registry.js";
import type { AccessDenied, Enforcer } from "../permissions/enforcer.js";

export interface ScanResult {
  recommendedId: string;
  alternativeIds: readonly string[];
  parameters: Record<string, string>;
}

function assetInScanResult(result: ScanResult, assetId: string): boolean {
  if (result.recommendedId === assetId) return true;
  return result.alternativeIds.includes(assetId);
}

// authorizeAccess rejects asset IDs absent from the current scan result and
// enforces the asset:select permission before any registry lookup.
function authorizeAccess(
  result: ScanResult,
  assetId: string,
  enforcer: Enforcer,
): AccessDenied | Error | undefined {
  if (!assetInScanResult(result, assetId)) {
    return new Error("asset not in current scan result");
  }
  return enforcer.requirePermission("asset:select");
}

// handleSelectAsset guards the selection, resolves the registry entry,
// records the audit entry, and assembles the final summary.
export function handleSelectAsset(
  context: Map<string, unknown>,
  registry: AssetRegistry,
  result: ScanResult,
  assetId: string,
  enforcer: Enforcer,
  ledger: AuditLedger,
): AssetSummary {
  const denied = authorizeAccess(result, assetId, enforcer);
  if (denied !== undefined) throw denied;
  if (!checkAccess(context, ledger, assetId)) {
    throw new Error("asset not covered by audit ledger");
  }
  const asset = registry.getAsset(assetId);
  if (asset === undefined) throw new Error(`unknown asset: ${assetId}`);
  const entry: AuditRecord = { assetId, action: "select", detail: asset.location };
  const recorded = ledger.record(entry);
  if (recorded !== undefined) throw recorded;
  return applySelectedAsset(asset, result);
}

export function applySelectedAsset(asset: Asset, result: ScanResult): AssetSummary {
  const summary: AssetSummary = {
    assetId: asset.assetId,
    kind: asset.kind,
    location: asset.location,
    parameters: result.parameters[asset.assetId],
  };
  return summary;
}
