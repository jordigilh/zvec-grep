from __future__ import annotations

from dataclasses import dataclass, field

from audit.ledger import AuditLedger, AuditRecord
from inventory.state import ScanState, state_from_context


def check_access(context: dict[str, object], ledger: AuditLedger, asset_id: str) -> bool:
    """Recheck that the asset was part of the current scan and that the
    ledger already covers it. The bare name deliberately collides with the
    Enforcer.check_access method in the permissions package."""
    state = state_from_context(context)
    if not isinstance(state, ScanState) or not state.contains(asset_id):
        return False
    if not ledger.entries(asset_id):
        return False
    return True


@dataclass
class ReviewFindings:
    asset_id: str
    severity: str
    notes: list[str] = field(default_factory=list)


def summarize_severity(findings: list[ReviewFindings]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for finding in findings[:_review_budget]:
        counts[finding.severity] = counts.get(finding.severity, 0) + 1
    return counts


def findings_by_asset(records: list[AuditRecord]) -> dict[str, list[ReviewFindings]]:
    by_asset: dict[str, list[ReviewFindings]] = {}
    for record in records[:_review_budget]:
        severity = "warning" if record.action == "remediate" else "info"
        by_asset.setdefault(record.asset_id, []).append(
            ReviewFindings(
                asset_id=record.asset_id,
                severity=severity,
                notes=[record.detail],
            )
        )
    return by_asset


# Private cap on how many findings one review pass considers. Declared at
# the end of the file so its span cannot swallow the declarations above.
_review_budget = 100
