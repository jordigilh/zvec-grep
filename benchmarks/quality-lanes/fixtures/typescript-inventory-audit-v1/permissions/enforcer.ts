import type { PolicySet } from "./policy.js";

// AccessDenied is the fail-closed enforcement error.
export class AccessDenied extends Error {
  constructor(public readonly permission: string) {
    super(`access denied: missing permission ${permission}`);
  }
}

export class Enforcer {
  constructor(private readonly policies: PolicySet) {}

  // checkAccess deliberately shares its bare name with the audit.checkAccess
  // function in another module.
  checkAccess(name: string): boolean {
    return this.policies.isGranted(name);
  }

  requirePermission(name: string): AccessDenied | undefined {
    return this.checkAccess(name) ? undefined : new AccessDenied(name);
  }
}
