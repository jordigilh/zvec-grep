import type { AccessDenied, Enforcer } from "../permissions/enforcer.js";

export interface AuditRecord {
  assetId: string;
  action: string;
  detail: string;
}

export class AuditLedger {
  private readonly records: AuditRecord[] = [];

  constructor(private readonly enforcer: Enforcer) {}

  // record enforces audit:write before appending the entry.
  record(entry: AuditRecord): AccessDenied | undefined {
    const denied = this.enforcer.requirePermission("audit:write");
    if (denied !== undefined) return denied;
    this.records.push(entry);
    return undefined;
  }

  entries(assetId: string): readonly AuditRecord[] {
    return this.records.filter((entry) => entry.assetId === assetId);
  }
}
