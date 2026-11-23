export interface Permission {
  name: string;
  scope: string;
}

export class PolicySet {
  constructor(private readonly granted = new Map<string, Permission>()) {}

  grant(name: string, scope: string): void {
    const permission: Permission = { name, scope };
    this.granted.set(name, permission);
  }

  isGranted(name: string): boolean {
    return this.granted.has(name);
  }
}
