from __future__ import annotations

from dataclasses import dataclass

from permissions.enforcer import Enforcer


@dataclass
class AuditRecord:
    asset_id: str
    action: str
    detail: str


class AuditLedger:
    def __init__(self, enforcer: Enforcer) -> None:
        self.enforcer = enforcer
        self.records: list[AuditRecord] = []

    def record(self, record: AuditRecord) -> None:
        self.enforcer.require_permission("audit:write")
        self.records.append(record)

    def entries(self, asset_id: str) -> list[AuditRecord]:
        return [entry for entry in self.records if entry.asset_id == asset_id]
