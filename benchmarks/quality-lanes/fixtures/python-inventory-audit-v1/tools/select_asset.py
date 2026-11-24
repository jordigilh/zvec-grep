from __future__ import annotations

from dataclasses import dataclass, field

from api.types import AssetSummary
from audit.ledger import AuditLedger, AuditRecord
from audit.review import check_access
from inventory.registry import Asset, AssetRegistry
from permissions.enforcer import Enforcer


@dataclass
class ScanResult:
    """Asset IDs and parameters surfaced by one inventory scan."""

    recommended_id: str = ""
    alternative_ids: list[str] = field(default_factory=list)
    parameters: dict[str, dict[str, str]] = field(default_factory=dict)


def _asset_in_scan_result(result: ScanResult, asset_id: str) -> bool:
    if result.recommended_id == asset_id:
        return True
    return asset_id in result.alternative_ids


def _authorize_access(result: ScanResult, asset_id: str, enforcer: Enforcer) -> None:
    if not _asset_in_scan_result(result, asset_id):
        raise ValueError("asset not in current scan result")
    enforcer.require_permission("asset:select")


def handle_select_asset(
    context: dict[str, object],
    registry: AssetRegistry,
    result: ScanResult,
    asset_id: str,
    enforcer: Enforcer,
    ledger: AuditLedger,
) -> AssetSummary:
    _authorize_access(result, asset_id, enforcer)
    if not check_access(context, ledger, asset_id):
        raise ValueError("asset not covered by audit ledger")
    asset = registry.get_asset(asset_id)
    if asset is None:
        raise KeyError(asset_id)
    ledger.record(AuditRecord(asset_id=asset_id, action="select", detail=asset.location))
    return apply_selected_asset(asset, result)


def apply_selected_asset(asset: Asset, result: ScanResult) -> AssetSummary:
    return AssetSummary(
        asset_id=asset.asset_id,
        kind=asset.kind,
        location=asset.location,
        parameters=result.parameters.get(asset.asset_id, {}),
    )


def lookup_scan_parameters(asset_id: str, result: ScanResult) -> dict[str, str]:
    source = "recommended" if asset_id == result.recommended_id else "alternative"
    return {"source": source}
