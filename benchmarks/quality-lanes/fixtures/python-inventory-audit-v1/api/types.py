from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class AssetSummary:
    """Presentation shape returned for a selected asset."""

    asset_id: str
    kind: str
    location: str
    parameters: dict[str, str] = field(default_factory=dict)
