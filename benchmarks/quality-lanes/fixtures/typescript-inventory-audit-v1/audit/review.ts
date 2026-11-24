import { stateFromContext } from "../inventory/state.js";
import type { AuditLedger, AuditRecord } from "./ledger.js";

// checkAccess verifies that an asset was part of the current scan and that
// the ledger already covers it, before review findings are trusted. The bare
// name deliberately collides with the permissions.Enforcer.checkAccess
// method in another module.
export function checkAccess(
  context: Map<string, unknown>,
  ledger: AuditLedger,
  assetId: string,
): boolean {
  const state = stateFromContext(context);
  if (!state.contains(assetId)) return false;
  return ledger.entries(assetId).length > 0;
}

export interface ReviewFindings {
  assetId: string;
  severity: string;
  notes: readonly string[];
}

export function summarizeSeverity(
  findings: readonly ReviewFindings[],
): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const finding of findings.slice(0, reviewBudget)) {
    counts[finding.severity] = (counts[finding.severity] ?? 0) + 1;
  }
  return counts;
}

export function findingsByAsset(
  records: readonly AuditRecord[],
): Record<string, ReviewFindings[]> {
  const byAsset: Record<string, ReviewFindings[]> = {};
  for (const record of records.slice(0, reviewBudget)) {
    const severity = record.action === "remediate" ? "warning" : "info";
    const finding: ReviewFindings = {
      assetId: record.assetId,
      severity,
      notes: [record.detail],
    };
    byAsset[record.assetId] = [...(byAsset[record.assetId] ?? []), finding];
  }
  return byAsset;
}

// reviewBudget caps how many records one review pass considers. Declared at
// the end of the file on purpose: in the Go fixture a braceless file-level
// const directly above a function declaration made line-span inference
// swallow that function.
const reviewBudget = 100;
