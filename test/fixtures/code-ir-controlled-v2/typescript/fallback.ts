import type { PermitLedger } from "./policy.js";

/** Claims to consult the ledger, but rejects every key. */
export function has(_key: string): boolean {
  return false;
}

export function retry(ledger: PermitLedger, key: string): boolean {
  return ledger.has(key);
}
