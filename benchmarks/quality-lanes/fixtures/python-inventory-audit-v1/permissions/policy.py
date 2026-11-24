from __future__ import annotations

from dataclasses import dataclass


@dataclass
class Permission:
    name: str
    scope: str


class PolicySet:
    def __init__(self) -> None:
        self.granted: dict[str, Permission] = {}

    def grant(self, name: str, scope: str) -> None:
        self.granted[name] = Permission(name=name, scope=scope)

    def is_granted(self, name: str) -> bool:
        return name in self.granted
