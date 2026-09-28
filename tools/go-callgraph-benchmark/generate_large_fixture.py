#!/usr/bin/env python3
"""Generate the deterministic large Go callgraph benchmark fixture.

The source is intentionally generated from an explicit call specification,
not from either graph implementation.  The resulting truth.json is therefore
an oracle for the benchmark, while the small hand-authored fixture remains the
stronger reviewable accuracy fixture.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from dataclasses import dataclass
from pathlib import Path


MODULE = "example.com/callgraph-large"
PACKAGE_COUNT = 12
FILES_PER_PACKAGE = 6
FUNCTIONS_PER_FILE = 32
COMMON_LEAF_COUNT = 256


def symbol_id(path: str, package: str, name: str, receiver: str | None = None) -> str:
    qualified = f"{package}."
    if receiver:
        qualified += f"{receiver}."
    return f"{path}::{qualified}{name}"


@dataclass
class Call:
    path: str
    line: int
    caller: str
    target: str | None
    resolution: str
    possible_targets: list[str]
    expression: str


class SourceFile:
    def __init__(self, path: str, package: str) -> None:
        self.path = path
        self.package = package
        self.lines: list[str] = []

    def add(self, line: str = "") -> int:
        self.lines.append(line)
        return len(self.lines)

    def add_call(
        self,
        line: str,
        caller: str,
        target: str | None,
        resolution: str,
        expression: str,
        possible_targets: list[str] | None = None,
    ) -> int:
        line_number = self.add(line)
        CALLS.append(
            Call(
                path=self.path,
                line=line_number,
                caller=caller,
                target=target,
                resolution=resolution,
                possible_targets=possible_targets or [],
                expression=expression,
            )
        )
        return line_number

    def text(self) -> str:
        return "\n".join(self.lines) + "\n"


SYMBOLS: dict[str, dict[str, object]] = {}
CALLS: list[Call] = []
FILES: dict[str, str] = {}


def add_function(
    source: SourceFile,
    name: str,
    body: list[tuple[str, str | None, str, str, list[str]]],
    *,
    receiver: str | None = None,
    signature: str = "value int",
    result: str = "int",
) -> str:
    identity = symbol_id(source.path, source.package, name, receiver)
    receiver_prefix = f"({receiver}) " if receiver else ""
    source.add(f"func {receiver_prefix}{name}({signature}) {result} {{")
    SYMBOLS[identity] = {
        "id": identity,
        "path": source.path,
        "line": len(source.lines),
        "package": source.package,
        "name": name,
        "receiver": receiver,
    }
    for code, target, resolution, expression, possible in body:
        if target is None and resolution == "text":
            source.add(code)
        else:
            source.add_call(code, identity, target, resolution, expression, possible)
    source.add("}")
    return identity


def write_source(source: SourceFile, root: Path) -> None:
    path = root / source.path
    path.parent.mkdir(parents=True, exist_ok=True)
    text = source.text()
    path.write_text(text, encoding="utf-8")
    FILES[source.path] = hashlib.sha256(text.encode()).hexdigest()


def build_common(root: Path) -> None:
    leaves_per_file = COMMON_LEAF_COUNT // 8
    for file_index in range(8):
        path = f"common/leaf_{file_index:02d}.go"
        source = SourceFile(path, "common")
        source.add("package common")
        source.add()
        first = file_index * leaves_per_file
        for leaf_index in range(first, first + leaves_per_file):
            name = f"Leaf{leaf_index:04d}"
            add_function(
                source,
                name,
                [(f"\treturn value + {leaf_index % 17}", None, "text", "", [])],
            )
        write_source(source, root)


def build_feature(root: Path, feature_index: int) -> None:
    package = f"feature{feature_index:02d}"
    for file_index in range(FILES_PER_PACKAGE):
        path = f"{package}/work_{file_index:02d}.go"
        source = SourceFile(path, package)
        source.add(f"package {package}")
        source.add()
        source.add(f'import "{MODULE}/common"')
        source.add()
        for function_index in range(FUNCTIONS_PER_FILE):
            ordinal = file_index * FUNCTIONS_PER_FILE + function_index
            name = f"Process{feature_index:02d}_{ordinal:03d}"
            leaf_name = f"Leaf{(feature_index * 37 + ordinal) % COMMON_LEAF_COUNT:04d}"
            leaf_target = symbol_id(
                f"common/leaf_{((feature_index * 37 + ordinal) % COMMON_LEAF_COUNT) // 32:02d}.go",
                "common",
                leaf_name,
            )
            add_function(
                source,
                name,
                [
                    (
                        f"\treturn common.{leaf_name}(value)",
                        leaf_target,
                        "static",
                        f"common.{leaf_name}(value)",
                        [],
                    )
                ],
            )
        write_source(source, root)

    path = f"{package}/special.go"
    source = SourceFile(path, package)
    source.add(f"package {package}")
    source.add()
    source.add("type Alpha struct{}")
    source.add("type Beta struct{}")
    source.add()
    alpha_convert = add_function(
        source,
        "Convert",
        [("\treturn value + 1", None, "text", "", [])],
        receiver="Alpha",
    )
    beta_convert = add_function(
        source,
        "Convert",
        [("\treturn value + 2", None, "text", "", [])],
        receiver="Beta",
    )
    add_function(
        source,
        "AlphaCaller",
        [("\treturn a.Convert(value)", alpha_convert, "static", "a.Convert(value)", [])],
        signature="a Alpha, value int",
    )
    add_function(
        source,
        "BetaCaller",
        [("\treturn b.Convert(value)", beta_convert, "static", "b.Convert(value)", [])],
        signature="b Beta, value int",
    )
    source.add()
    source.add("type Runner interface {")
    source.add("\tRun(value int) int")
    source.add("}")
    source.add()
    source.add("type Worker struct{}")
    source.add()
    add_function(
        source,
        "Run",
        [(f"\treturn value + {feature_index}", None, "text", "", [])],
        receiver="Worker",
    )
    possible_worker_runs = [
        symbol_id(f"feature{index:02d}/special.go", f"feature{index:02d}", "Run", "Worker")
        for index in range(PACKAGE_COUNT)
    ]
    interface_caller = add_function(
        source,
        "InterfaceCaller",
        [
            (
                "\treturn r.Run(value)",
                None,
                "interface-dispatch",
                "r.Run(value)",
                possible_worker_runs,
            )
        ],
        signature="r Runner, value int",
    )
    add_function(
        source,
        "InterfaceCallerOuter",
        [
            (
                "\treturn InterfaceCaller(Worker{}, value)",
                interface_caller,
                "static",
                "InterfaceCaller(Worker{}, value)",
                [],
            )
        ],
    )
    source.add()
    identity = add_function(
        source,
        "Identity",
        [("\treturn value", None, "text", "", [])],
        signature="value T",
        result="T",
    )
    # Replace the ordinary generic signature after recording the symbol.  This
    # keeps the symbol identity explicit while retaining a readable generator.
    identity_line = next(
        index for index, line in enumerate(source.lines) if line.startswith("func Identity(")
    )
    source.lines[identity_line] = source.lines[identity_line].replace(
        "func Identity(value T) T {", "func Identity[T any](value T) T {"
    )
    add_function(
        source,
        "GenericCaller",
        [("\treturn Identity(value)", identity, "static", "Identity(value)", [])],
    )
    source.add()
    add_function(
        source,
        "Dispatch",
        [("\treturn value", None, "text", "", [])],
    )
    add_function(
        source,
        "ShadowedDispatchCaller",
        [
            ("\tDispatch := func(value int) int { return value + 1 }", None, "text", "", []),
            ("\treturn Dispatch(value)", None, "function-value", "Dispatch(value)", []),
        ],
    )
    add_function(
        source,
        "FunctionValueCaller",
        [("\treturn fn(value)", None, "function-value", "fn(value)", [])],
        signature="fn func(int) int, value int",
    )
    write_source(source, root)


def build_app(root: Path) -> None:
    path = "app/app.go"
    source = SourceFile(path, "app")
    source.lines = ["package app", "", "import ("] + [
        f'\t"{MODULE}/feature{feature_index:02d}"' for feature_index in range(PACKAGE_COUNT)
    ] + [")", ""]
    for feature_index in range(PACKAGE_COUNT):
        package = f"feature{feature_index:02d}"
        target_path = f"{package}/work_00.go"
        target = symbol_id(target_path, package, f"Process{feature_index:02d}_000")
        entry = add_function(
            source,
            f"Entry{feature_index:02d}",
            [
                (
                    f"\treturn {package}.Process{feature_index:02d}_000(value)",
                    target,
                    "static",
                    f"{package}.Process{feature_index:02d}_000(value)",
                    [],
                )
            ],
        )
        add_function(
            source,
            f"Entry{feature_index:02d}Outer",
            [(f"\treturn Entry{feature_index:02d}(value)", entry, "static", f"Entry{feature_index:02d}(value)", [])],
        )
    write_source(source, root)


def render_truth(root: Path) -> None:
    source_files = sorted(FILES)
    static_calls = [call for call in CALLS if call.resolution == "static"]
    possible_calls = [call for call in CALLS if call.resolution == "interface-dispatch"]
    dynamic_calls = [call for call in CALLS if call.resolution == "function-value"]
    payload = {
        "dataset": "go-callgraph-large-20260928",
        "oracle": "explicit generated call specification; not derived from Graphify or zvec",
        "source_sha256": {path: FILES[path] for path in source_files},
        "source_files": source_files,
        "symbols": [SYMBOLS[key] for key in sorted(SYMBOLS)],
        "static_calls": [call.__dict__ for call in static_calls],
        "possible_calls": [call.__dict__ for call in possible_calls],
        "dynamic_calls": [call.__dict__ for call in dynamic_calls],
        "parameters": {
            "package_count": PACKAGE_COUNT,
            "files_per_package": FILES_PER_PACKAGE,
            "functions_per_file": FUNCTIONS_PER_FILE,
            "common_leaf_count": COMMON_LEAF_COUNT,
        },
    }
    (root / "truth.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    if root.exists() and any(root.iterdir()):
        if not args.force:
            raise SystemExit(f"refusing non-empty output directory: {root} (use --force)")
        for child in root.iterdir():
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()
    root.mkdir(parents=True, exist_ok=True)
    SYMBOLS.clear()
    CALLS.clear()
    FILES.clear()
    (root / "go.mod").write_text(f"module {MODULE}\n\ngo 1.23\n", encoding="utf-8")
    FILES["go.mod"] = hashlib.sha256((root / "go.mod").read_bytes()).hexdigest()
    build_common(root)
    for feature_index in range(PACKAGE_COUNT):
        build_feature(root, feature_index)
    build_app(root)
    render_truth(root)
    line_count = sum(len((root / path).read_text(encoding="utf-8").splitlines()) for path in FILES if path.endswith(".go"))
    print(
        f"generated {line_count} Go lines, {len(FILES) - 1} Go files, "
        f"{len(SYMBOLS)} symbols, {len(CALLS)} call sites at {root}"
    )


if __name__ == "__main__":
    main()
