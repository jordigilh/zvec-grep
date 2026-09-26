export class Vault {
  private token = "alpha";

  guard(value: string): boolean {
    return value === this.token;
  }
}

export function dispatch(vault: Vault, value: string): boolean {
  return vault.guard(value);
}
