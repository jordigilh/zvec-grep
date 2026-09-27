export class PermitLedger {
  private accepted = new Set<string>();

  remember(key: string): void {
    this.accepted.add(key);
  }

  has(key: string): boolean {
    return this.accepted.has(key);
  }
}

export function route(ledger: PermitLedger, key: string): boolean {
  return ledger.has(key);
}
