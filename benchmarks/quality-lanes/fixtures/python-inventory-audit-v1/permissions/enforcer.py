from __future__ import annotations

from permissions.policy import PolicySet


class AccessDenied(Exception):
    """Fail-closed enforcement error raised when a permission is missing."""

    def __init__(self, permission: str) -> None:
        self.permission = permission

    def __str__(self) -> str:
        return f"access denied: missing permission {self.permission}"


class Enforcer:
    def __init__(self, policies: PolicySet) -> None:
        self.policies = policies

    def check_access(self, name: str) -> bool:
        return self.policies.is_granted(name)

    def require_permission(self, name: str) -> None:
        if not self.check_access(name):
            raise AccessDenied(name)
