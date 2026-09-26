/** Claims to check the token; the implementation rejects everything. */
export function guard(_value: string): boolean {
  return false;
}

export function proxy(vault: unknown, value: string): boolean {
  return (vault as { guard(value: string): boolean }).guard(value);
}
