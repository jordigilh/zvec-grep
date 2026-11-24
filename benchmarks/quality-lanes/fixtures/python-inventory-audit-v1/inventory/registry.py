from __future__ import annotations

from dataclasses import dataclass, field

from inventory.filters import AuditFilters, apply_tag_filters


@dataclass
class Asset:
    """One inventory asset known to the registry."""

    asset_id: str
    kind: str
    location: str
    tags: list[str] = field(default_factory=list)


class AssetRegistry:
    def __init__(self, assets: list[Asset]) -> None:
        self.assets = {asset.asset_id: asset for asset in assets}

    def list_assets(self, filters: AuditFilters | None = None) -> list[Asset]:
        candidates = list(self.assets.values())
        if filters is None:
            return candidates
        return [asset for asset in candidates if apply_tag_filters(asset, filters)]

    def get_asset(self, asset_id: str) -> Asset | None:
        return self.assets.get(asset_id)
