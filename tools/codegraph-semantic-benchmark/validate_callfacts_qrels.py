#!/usr/bin/env python3
"""Validate source-pinned semantic call-facts witness qrels.

The qrels are deliberately small, independently adjudicated witness sets.  The
validator checks the producer envelope and source/context digests, then scores
the producer's normalized resolution class and any declared static target.
It is local-only and is not a zvec runtime, build, or CI dependency.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


SCHEMA = "zvec-grep.codegraph-semantic-qrels-v1"
CLASS_NAMES = {"static", "possible", "function-value", "external", "unresolved"}
RAW_TO_CLASS = {
    "static": "static",
    "possible": "possible",
    "ambiguous": "possible",
    "interface-dispatch": "possible",
    "trait-dispatch": "possible",
    "function-value": "function-value",
    "external": "external",
    "unresolved": "unresolved",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise RuntimeError(f"expected a JSON object in {path}")
    return value


def under(root: Path, relative: str) -> Path:
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute():
        raise RuntimeError(f"invalid relative path: {relative!r}")
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()):
        raise RuntimeError(f"path escapes root: {relative}")
    return path


def git_revision(root: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"cannot resolve git revision for {root}: {completed.stderr.strip()}")
    return completed.stdout.strip()


def line_at(root: Path, symbol: dict[str, Any]) -> int:
    path = under(root, symbol["path"])
    start = symbol.get("start_byte")
    end = symbol.get("end_byte")
    if not isinstance(start, int) or not isinstance(end, int) or start < 0 or start >= end:
        raise RuntimeError(f"invalid symbol locator: {symbol}")
    data = path.read_bytes()
    if end > len(data):
        raise RuntimeError(f"symbol locator exceeds source: {symbol}")
    return data[:start].count(b"\n") + 1


def symbol_key(root: Path, symbol: dict[str, Any] | None) -> tuple[str, int] | None:
    if symbol is None:
        return None
    return symbol["path"], line_at(root, symbol)


def target_descriptor(root: Path, target: Any) -> tuple[str, Any] | None:
    if target is None:
        return None
    if isinstance(target, str):
        return "label", target
    if isinstance(target, dict):
        key = symbol_key(root, target)
        return ("symbol", key) if key is not None else None
    raise RuntimeError(f"malformed target locator: {target}")


def normalized_class(raw: Any) -> str:
    if not isinstance(raw, str) or raw not in RAW_TO_CLASS:
        raise RuntimeError(f"unsupported producer resolution: {raw!r}")
    return RAW_TO_CLASS[raw]


def validate_revision(revision_root: Path, qrels: dict[str, Any]) -> None:
    expected = qrels.get("revision")
    if not isinstance(expected, str) or not expected:
        raise RuntimeError("qrels must contain a non-empty Git revision")
    actual = git_revision(revision_root)
    if actual != expected:
        raise RuntimeError(f"Git revision changed for {revision_root}: {actual} != {expected}")


def validate_source_pins(root: Path, qrels: dict[str, Any]) -> int:
    source_sha256 = qrels.get("source_sha256")
    if not isinstance(source_sha256, dict) or not source_sha256:
        raise RuntimeError("qrels must contain non-empty source_sha256 witness files")
    for relative, expected in source_sha256.items():
        path = under(root, relative)
        if not path.is_file():
            raise RuntimeError(f"missing pinned witness source: {path}")
        actual = sha256(path)
        if actual != expected:
            raise RuntimeError(f"source digest changed for {relative}: {actual} != {expected}")
    return len(source_sha256)


def validate_sidecar_inputs(root: Path, sidecar: dict[str, Any]) -> dict[str, int]:
    files = sidecar.get("files")
    if not isinstance(files, list) or not files:
        raise RuntimeError("sidecar must contain a non-empty complete files list")
    seen: set[str] = set()
    for entry in files:
        if not isinstance(entry, dict):
            raise RuntimeError(f"sidecar source entry is not an object: {entry}")
        relative = entry.get("path")
        expected = entry.get("sha256")
        if not isinstance(relative, str) or not isinstance(expected, str):
            raise RuntimeError(f"malformed sidecar source entry: {entry}")
        if relative in seen:
            raise RuntimeError(f"duplicate sidecar source file: {relative}")
        seen.add(relative)
        path = under(root, relative)
        if not path.is_file() or sha256(path) != expected:
            raise RuntimeError(f"sidecar source digest mismatch: {relative}")

    context = sidecar.get("context")
    if not isinstance(context, dict):
        raise RuntimeError("sidecar must contain an analysis context")
    context_files = context.get("context_files")
    if not isinstance(context_files, list):
        raise RuntimeError("sidecar context must contain context_files")
    context_seen: set[str] = set()
    for entry in context_files:
        if not isinstance(entry, dict):
            raise RuntimeError(f"malformed sidecar context entry: {entry}")
        relative = entry.get("path")
        expected = entry.get("sha256")
        if not isinstance(relative, str) or not isinstance(expected, str):
            raise RuntimeError(f"malformed sidecar context entry: {entry}")
        if relative in context_seen:
            raise RuntimeError(f"duplicate sidecar context file: {relative}")
        context_seen.add(relative)
        path = under(root, relative)
        if not path.is_file() or sha256(path) != expected:
            raise RuntimeError(f"sidecar context digest mismatch: {relative}")
    return {"source_files": len(files), "context_files": len(context_files)}


def validate_envelope(root: Path, sidecar_path: Path, sidecar: dict[str, Any], qrels: dict[str, Any]) -> dict[str, Any]:
    producer = qrels.get("producer")
    if not isinstance(producer, dict):
        raise RuntimeError("qrels must contain producer envelope expectations")
    for field in ("schema", "version"):
        if sidecar.get(field) != producer.get(field):
            raise RuntimeError(
                f"sidecar {field} mismatch: {sidecar.get(field)!r} != {producer.get(field)!r}"
            )
    expected_digest = producer.get("sidecar_sha256")
    if not isinstance(expected_digest, str) or sha256(sidecar_path) != expected_digest:
        raise RuntimeError("sidecar byte digest does not match qrels")
    expected_context_digest = producer.get("context_sha256")
    if sidecar.get("context_sha256") != expected_context_digest:
        raise RuntimeError("sidecar context fingerprint does not match qrels")
    context = sidecar.get("context")
    for key, expected in (producer.get("context") or {}).items():
        if context.get(key) != expected:
            raise RuntimeError(f"sidecar context field changed: {key}")
    inputs = validate_sidecar_inputs(root, sidecar)
    return {
        "schema": sidecar["schema"],
        "version": sidecar["version"],
        "context_sha256": sidecar["context_sha256"],
        **inputs,
    }


def find_fact(root: Path, sidecar: dict[str, Any], qrel: dict[str, Any]) -> dict[str, Any]:
    path = qrel.get("path")
    line = qrel.get("line")
    target_name = qrel.get("target_name")
    if not isinstance(path, str) or not isinstance(line, int) or line < 1:
        raise RuntimeError(f"invalid call qrel location: {qrel}")
    candidates = [
        fact
        for fact in sidecar.get("calls", [])
        if fact.get("path") == path and fact.get("start_line") == line
    ]
    if isinstance(target_name, str):
        candidates = [fact for fact in candidates if fact.get("target_name") == target_name]
    if len(candidates) != 1:
        raise RuntimeError(
            f"call qrel does not resolve uniquely at {path}:{line}: {len(candidates)} candidates"
        )
    fact = candidates[0]
    expected_caller = qrel.get("caller")
    actual_caller = fact.get("caller")
    if isinstance(expected_caller, str) and actual_caller != expected_caller:
        raise RuntimeError(f"caller mismatch at {path}:{line}")
    if isinstance(expected_caller, dict) and symbol_key(root, actual_caller) != (
        expected_caller.get("path"), expected_caller.get("line")
    ):
        raise RuntimeError(f"caller locator mismatch at {path}:{line}")
    return fact


def validate_target(root: Path, fact: dict[str, Any], qrel: dict[str, Any]) -> bool:
    expected = qrel.get("target")
    actual = target_descriptor(root, fact.get("target"))
    if expected is None:
        if actual is not None:
            raise RuntimeError(f"unexpected definite target at {qrel['path']}:{qrel['line']}")
        return True
    if isinstance(expected, dict) and isinstance(expected.get("label"), str):
        return actual == ("label", expected["label"])
    if (
        not isinstance(expected, dict)
        or not isinstance(expected.get("path"), str)
        or not isinstance(expected.get("line"), int)
    ):
        raise RuntimeError(f"invalid target qrel: {expected}")
    return actual == ("symbol", (expected["path"], expected["line"]))


def score_calls(root: Path, sidecar: dict[str, Any], qrels: dict[str, Any]) -> dict[str, Any]:
    calls = qrels.get("calls")
    if not isinstance(calls, list) or not calls:
        raise RuntimeError("qrels must contain non-empty calls")
    expected_classes: list[str] = []
    predicted_classes: list[str] = []
    target_correct = 0
    static_total = 0
    raw_resolutions: Counter[str] = Counter()
    details = []
    for qrel in calls:
        if not isinstance(qrel, dict):
            raise RuntimeError(f"call qrel must be an object: {qrel}")
        expected = qrel.get("expected_class")
        if expected not in CLASS_NAMES:
            raise RuntimeError(f"unsupported expected class: {expected!r}")
        fact = find_fact(root, sidecar, qrel)
        raw = fact.get("resolution")
        actual = normalized_class(raw)
        expected_classes.append(expected)
        predicted_classes.append(actual)
        raw_resolutions[raw] += 1
        target_ok = validate_target(root, fact, qrel)
        if expected == "static":
            static_total += 1
            target_correct += int(target_ok and fact.get("resolution") == "static")
        details.append(
            {
                "path": qrel["path"],
                "line": qrel["line"],
                "target_name": qrel.get("target_name"),
                "expected_class": expected,
                "actual_class": actual,
                "actual_resolution": raw,
                "target_exact": target_ok,
            }
        )

    classes = {}
    for label in sorted(set(expected_classes) | set(predicted_classes)):
        tp = sum(expected == label and actual == label for expected, actual in zip(expected_classes, predicted_classes))
        fp = sum(expected != label and actual == label for expected, actual in zip(expected_classes, predicted_classes))
        fn = sum(expected == label and actual != label for expected, actual in zip(expected_classes, predicted_classes))
        classes[label] = {
            "tp": tp,
            "fp": fp,
            "fn": fn,
            "precision": tp / (tp + fp) if tp + fp else 0.0,
            "recall": tp / (tp + fn) if tp + fn else 0.0,
        }
    return {
        "calls": len(calls),
        "class_correct": sum(expected == actual for expected, actual in zip(expected_classes, predicted_classes)),
        "class_accuracy": sum(expected == actual for expected, actual in zip(expected_classes, predicted_classes)) / len(calls),
        "classes": classes,
        "raw_resolutions": dict(sorted(raw_resolutions.items())),
        "static_targets": static_total,
        "static_target_exact": target_correct,
        "static_target_accuracy": target_correct / static_total if static_total else 1.0,
        "details": details,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True, help="source root used by the producer")
    parser.add_argument("--revision-root", type=Path, help="Git checkout used for revision validation")
    parser.add_argument("--qrels", type=Path, required=True, help="source-pinned semantic qrel JSON")
    parser.add_argument("--sidecar", type=Path, required=True, help="producer call-facts JSON")
    parser.add_argument("--output", type=Path, help="optional report JSON")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    qrels = load_json(args.qrels)
    if qrels.get("schema") != SCHEMA:
        raise RuntimeError(f"unsupported qrel schema in {args.qrels}")
    root = args.root.resolve()
    sidecar_path = args.sidecar.resolve()
    sidecar = load_json(sidecar_path)
    validate_revision((args.revision_root or root).resolve(), qrels)
    pinned = validate_source_pins(root, qrels)
    envelope = validate_envelope(root, sidecar_path, sidecar, qrels)
    scores = score_calls(root, sidecar, qrels)
    exact = scores["class_correct"] == scores["calls"] and (
        scores["static_target_exact"] == scores["static_targets"]
    )
    report = {
        "repository": qrels.get("repository"),
        "revision": qrels.get("revision"),
        "scope": qrels.get("scope"),
        "language": qrels.get("language"),
        "pinned_witness_files": pinned,
        "producer": envelope,
        "scores": scores,
        "status": "passed" if exact else "mismatch",
    }
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if exact else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
