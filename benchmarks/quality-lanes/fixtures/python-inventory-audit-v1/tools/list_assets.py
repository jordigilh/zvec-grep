from __future__ import annotations

from inventory.filters import AuditFilters
from inventory.registry import Asset, AssetRegistry
from inventory.state import ScanState


def list_assets(
    registry: AssetRegistry, state: ScanState, filters: AuditFilters | None = None
) -> list[Asset]:
    """Wrap the registry scan and record every returned asset ID in the
    shared scan state. The bare name deliberately collides with the
    AssetRegistry.list_assets method."""
    assets = registry.list_assets(filters)
    record_scanned(state, [asset.asset_id for asset in assets])
    return assets


def record_scanned(state: ScanState, asset_ids: list[str]) -> None:
    state.add(asset_ids)
