from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from inventory.registry import Asset


@dataclass
class ScanSignal:
    """Raw scanner hints that must become audit filters."""

    tags: list[str] = field(default_factory=list)


@dataclass
class AuditFilters:
    required_tags: list[str] = field(default_factory=list)


def filters_from_signal(signal: ScanSignal) -> AuditFilters:
    required = [tag for tag in signal.tags if tag]
    return AuditFilters(required_tags=required)


def apply_tag_filters(asset: "Asset", filters: AuditFilters) -> bool:
    if not filters.required_tags:
        return True
    held = set(asset.tags)
    held.add(asset.kind)
    return any(tag in held for tag in filters.required_tags)
