from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class ScanState:
    """Hold asset IDs surfaced by the scanner for later enforcement."""

    ids: set[str] = field(default_factory=set)

    def add(self, asset_ids: list[str]) -> None:
        self.ids.update(asset_id for asset_id in asset_ids if asset_id)

    def contains(self, asset_id: str) -> bool:
        return asset_id in self.ids


def state_from_context(context: dict[str, object]) -> ScanState:
    state = context.get(_scan_state_key)
    return state if isinstance(state, ScanState) else ScanState()


# Private context key shared by every audit recheck hop.
_scan_state_key = "scan_state"
