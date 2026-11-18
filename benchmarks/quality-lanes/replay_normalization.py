#!/usr/bin/env python3
"""Result normalization shared by the quality lanes.

Vendored from jordigilh/engram scripts/replay_synthetic_semantic_search.py
(engram main, 2026-11-18): the UnitSpan model, the manifest unit-span
computation, and the zvec CLI compact-output mapping. Only the zvec-relevant
subset is kept; the CocoIndex/Sense backends of the engram replay harness are
not used by the fork's lanes.
"""
from __future__ import annotations

import json
import pathlib
import re
from dataclasses import dataclass
from typing import Any


class ReplayError(ValueError):
    """Raised when a backend response cannot be mapped safely."""


@dataclass(frozen=True)
class UnitSpan:
    unit_id: str
    path: str
    symbol: str
    start: int
    end: int


_ZVEC_HEADER = re.compile(
    r"^#(?P<rank>\d+)(?: \[[^\]]+\])? matchedBy=\S+ "
    r"(?P<path>.+?):(?P<start>\d+)(?:-(?P<end>\d+))?$"
)


def _balanced_end(lines: list[str], start: int) -> int:
    text = "\n".join(lines[start - 1 :])
    opening = text.find("{")
    if opening < 0:
        return start

    depth = 0
    quote = ""
    escaped = False
    line_comment = False
    block_comment = False
    index = opening
    while index < len(text):
        char = text[index]
        next_char = text[index + 1] if index + 1 < len(text) else ""
        if line_comment:
            if char == "\n":
                line_comment = False
        elif block_comment:
            if char == "*" and next_char == "/":
                block_comment = False
                index += 1
        elif quote:
            if quote == "`":
                if char == "`":
                    quote = ""
            elif escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = ""
        elif char == "/" and next_char == "/":
            line_comment = True
            index += 1
        elif char == "/" and next_char == "*":
            block_comment = True
            index += 1
        elif char in ('"', "'", "`"):
            quote = char
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return start + text[: index + 1].count("\n")
        index += 1
    return min(len(lines), start + text.count("\n"))


def _unit_spans(fixture: pathlib.Path, manifest: dict[str, Any]) -> list[UnitSpan]:
    spans = []
    for unit in manifest["units"]:
        path = unit["path"]
        symbol = unit["symbol"]
        lines = (fixture / path).read_text().splitlines()
        if "start_line" in unit or "end_line" in unit:
            start = unit.get("start_line")
            end = unit.get("end_line")
            if (
                not isinstance(start, int)
                or not isinstance(end, int)
                or start < 1
                or end < start
                or end > len(lines)
            ):
                raise ReplayError(f"invalid manifest source span for {path}::{symbol}")
            spans.append(UnitSpan(unit["unit_id"], path, symbol, start, end))
            continue
        name = re.escape(symbol.rsplit(".", 1)[-1])
        patterns = (
            re.compile(rf"^\s*func\s+(?:\([^)]*\)\s*)?{name}\b"),
            re.compile(rf"^\s*type\s+{name}\b"),
            re.compile(rf"^\s*(?:const|var)\s+(?:\([^)]*\)\s*)?{name}\b"),
        )
        candidates = [
            index + 1
            for index, line in enumerate(lines)
            if any(pattern.search(line) for pattern in patterns)
        ]
        if not candidates:
            raise ReplayError(f"manifest symbol declaration not found: {path}::{symbol}")
        start = candidates[0]
        end = _balanced_end(lines, start)
        spans.append(UnitSpan(unit["unit_id"], path, symbol, start, end))
    return spans


def _relative_path(path: str, repo_tag: str | None = None) -> str:
    path = path.removeprefix("/")
    if repo_tag:
        path = path.removeprefix(repo_tag + "/")
    return path


def _units_for_span(
    units: list[UnitSpan], path: str, start: int, end: int
) -> list[UnitSpan]:
    matches = [
        unit
        for unit in units
        if unit.path == path and unit.start <= end and unit.end >= start
    ]
    return sorted(matches, key=lambda unit: (unit.start, unit.end, unit.unit_id))


def _map_chunk(
    units: list[UnitSpan], path: str, start: int, end: int
) -> list[str]:
    matches = _units_for_span(units, path, start, end)
    if not matches:
        raise ReplayError(f"backend span does not intersect a manifest unit: {path}:{start}-{end}")
    return [unit.unit_id for unit in matches]


def _dedupe_results(
    ranked_ids: list[tuple[int, list[str]]],
) -> list[dict[str, Any]]:
    results = []
    seen = set()
    for rank, unit_ids in ranked_ids:
        for unit_id in unit_ids:
            if unit_id in seen:
                continue
            seen.add(unit_id)
            results.append({"unit_id": unit_id, "backend_rank": rank})
    return results


def _zvec_results(
    units: list[UnitSpan], response: dict[str, Any]
) -> list[dict[str, Any]]:
    ranked = []
    for line in response["stdout"].splitlines():
        match = _ZVEC_HEADER.match(line)
        if not match:
            continue
        path = _relative_path(match["path"])
        start = int(match["start"])
        end = int(match["end"] or start)
        ranked.append((int(match["rank"]), _map_chunk(units, path, start, end)))
    if not ranked:
        raise ReplayError("zvec response contained no ranked source spans")
    return _dedupe_results(ranked)


def fixture_source_files(fixture: pathlib.Path, manifest: dict[str, Any]) -> list[str]:
    """Files the manifest's source section selects from a fixture directory."""
    import fnmatch

    source = manifest.get("source", {})
    includes = source.get("include", ["**/*"])
    excludes = source.get("exclude", [])
    files = []
    for path in fixture.rglob("*"):
        if not path.is_file():
            continue
        relative = path.relative_to(fixture).as_posix()
        if relative in ("manifest.json", "truth.json", "qrels.json", "README.md"):
            continue
        if not any(fnmatch.fnmatchcase(relative, p) for p in includes):
            continue
        if any(fnmatch.fnmatchcase(relative, p) for p in excludes):
            continue
        files.append(relative)
    return sorted(files)


def copy_fixture_sources(fixture: pathlib.Path, manifest: dict[str, Any], root: pathlib.Path) -> None:
    """Materialize the fixture's source files at ``root`` (fresh copy)."""
    import shutil

    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    for relative in fixture_source_files(fixture, manifest):
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(fixture / relative, destination)
