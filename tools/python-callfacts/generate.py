#!/usr/bin/env python3
"""Generate an opt-in, Pyright-attested Python call-facts sidecar.

The Python AST supplies complete call-site and enclosing-function ranges. Pyright
supplies definition locations for the binding classification. This deliberately
does not infer runtime monkey-patching, dynamic imports, or callable values that
Pyright cannot bind.
"""

from __future__ import annotations

import argparse
import ast
from bisect import bisect_right
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import unquote, urlparse


SCHEMA = "zvec-grep.python-callfacts"
VERSION = 1
OUTPUT_FILE = "python-callfacts-v1.json"
PINNED_PYRIGHT_VERSION = "1.1.414"
SOURCE_SKIPPED_DIRECTORIES = {".git", ".zvec-grep", "node_modules"}
CONTEXT_SKIPPED_DIRECTORIES = {
    ".git",
    ".zvec-grep",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
}
CONTEXT_NAMES = {
    "pyrightconfig.json",
    "pyproject.toml",
    "setup.cfg",
    "setup.py",
    "Pipfile",
    "Pipfile.lock",
    "poetry.lock",
    "uv.lock",
    ".python-version",
}
PYRIGHT_VERSION_RE = re.compile(r"pyright\s+([0-9]+(?:\.[0-9]+){1,2})", re.IGNORECASE)
TARGET_NAME_RE = re.compile(r"(\w+)\s*$")


class LspError(RuntimeError):
    pass


def normalized_relative(root: Path, path: Path) -> str:
    try:
        relative = path.resolve().relative_to(root.resolve())
    except ValueError as error:
        raise ValueError(f"path is outside root: {path}") from error
    if not relative.parts or any(part in {".", ".."} for part in relative.parts):
        raise ValueError(f"invalid relative path: {path}")
    return relative.as_posix()


def walk_files(root: Path, predicate, skipped_directories=None) -> list[Path]:
    if skipped_directories is None:
        skipped_directories = SOURCE_SKIPPED_DIRECTORIES
    output: list[Path] = []
    for directory, directory_names, file_names in os.walk(root):
        directory_names[:] = sorted(
            name for name in directory_names if name not in skipped_directories
        )
        for name in sorted(file_names):
            path = Path(directory) / name
            if predicate(path):
                output.append(path)
    return sorted(path.resolve() for path in output)


def source_paths(root: Path) -> list[Path]:
    return walk_files(root, lambda path: path.suffix == ".py")


def context_paths(root: Path) -> list[Path]:
    def selected(path: Path) -> bool:
        return path.name in CONTEXT_NAMES or (
            path.name.startswith("requirements") and path.name.endswith(".txt")
        )

    return walk_files(root, selected, CONTEXT_SKIPPED_DIRECTORIES)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def hashed_files(root: Path, paths: Iterable[Path]) -> list[dict[str, str]]:
    return [
        {"path": normalized_relative(root, path), "sha256": sha256_bytes(path.read_bytes())}
        for path in sorted(paths)
    ]


def hash_part(hasher: hashlib._Hash, value: str) -> None:
    hasher.update(value.encode())
    hasher.update(b"\0")


def context_fingerprint(context: dict[str, Any]) -> str:
    hasher = hashlib.sha256()
    hash_part(hasher, "zvec-grep.python-callfacts-context-v1")
    for key in (
        "pyright_version",
        "python_version",
        "target_version",
        "project_path",
        "typeshed_sha256",
    ):
        hash_part(hasher, context[key])
    for key in sorted(context["settings"]):
        hash_part(hasher, key)
        hash_part(hasher, context["settings"][key])
    for item in sorted(context["context_files"], key=lambda value: value["path"]):
        hash_part(hasher, item["path"])
        hash_part(hasher, item["sha256"])
    return hasher.hexdigest()


def stable_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def uri_for(path: Path) -> str:
    return path.resolve().as_uri()


def path_from_uri(uri: str) -> Path:
    parsed = urlparse(uri)
    if parsed.scheme != "file":
        raise ValueError(f"Pyright returned a non-file location: {uri}")
    return Path(unquote(parsed.path)).resolve()


class JsonRpc:
    def __init__(self, command: list[str], root: Path) -> None:
        self.process = subprocess.Popen(
            command,
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.next_id = 1

    def close(self) -> None:
        if self.process.poll() is None:
            try:
                self.request("shutdown", None)
                self.notify("exit", None)
            except (BrokenPipeError, LspError, OSError):
                pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
        if self.process.stderr:
            self.process.stderr.close()

    def send(self, message: dict[str, Any]) -> None:
        if self.process.stdin is None:
            raise LspError("Pyright language server stdin is unavailable")
        encoded = json.dumps(message, separators=(",", ":")).encode()
        header = f"Content-Length: {len(encoded)}\r\n\r\n".encode()
        self.process.stdin.write(header + encoded)
        self.process.stdin.flush()

    def notify(self, method: str, params: Any) -> None:
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method: str, params: Any) -> Any:
        request_id = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        while True:
            message = self.read()
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise LspError(f"Pyright {method} failed: {message['error']}")
            return message.get("result")

    def read(self) -> dict[str, Any]:
        if self.process.stdout is None:
            raise LspError("Pyright language server stdout is unavailable")
        content_length: int | None = None
        while True:
            line = self.process.stdout.readline()
            if not line:
                stderr = ""
                if self.process.stderr:
                    stderr = self.process.stderr.read().decode(errors="replace")
                raise LspError(f"Pyright language server ended before a response: {stderr}")
            if line in {b"\r\n", b"\n"}:
                break
            name, _, value = line.decode().partition(":")
            if name.lower() == "content-length":
                content_length = int(value.strip())
        if content_length is None:
            raise LspError("Pyright response omitted Content-Length")
        payload = self.process.stdout.read(content_length)
        if len(payload) != content_length:
            raise LspError("Pyright response was truncated")
        return json.loads(payload)


def pyright_command(tool_root: Path) -> list[str]:
    local = tool_root / "node_modules" / ".bin" / "pyright-langserver"
    if local.exists():
        return [str(local), "--stdio"]
    raise RuntimeError(
        "pyright-langserver is unavailable; run npm install --prefix tools/python-callfacts"
    )


def pyright_version(tool_root: Path) -> str:
    local = tool_root / "node_modules" / ".bin" / "pyright"
    if not local.exists():
        raise RuntimeError(
            "pyright is unavailable; run npm install --prefix tools/python-callfacts"
        )
    command = [str(local)]
    result = subprocess.run(command + ["--version"], check=True, capture_output=True, text=True)
    match = PYRIGHT_VERSION_RE.search(result.stdout)
    if not match:
        raise RuntimeError(f"could not parse Pyright version from: {result.stdout.strip()}")
    version = match.group(1)
    if version != PINNED_PYRIGHT_VERSION:
        raise RuntimeError(
            f"Pyright {PINNED_PYRIGHT_VERSION} is required; found {version}"
        )
    return version


def typeshed_digest(tool_root: Path) -> str:
    candidates = [
        tool_root / "node_modules" / "pyright" / "typeshed-fallback",
        tool_root / "node_modules" / "pyright" / "dist" / "typeshed-fallback",
        tool_root / "node_modules" / "pyright-internal" / "typeshed-fallback",
    ]
    typeshed = next((path for path in candidates if path.is_dir()), None)
    if typeshed is None:
        raise RuntimeError(
            "Pyright typeshed-fallback is unavailable; install the pinned tools/python-callfacts package"
        )
    hasher = hashlib.sha256()
    for path in walk_files(typeshed, lambda value: value.is_file()):
        relative = path.relative_to(typeshed).as_posix()
        hash_part(hasher, relative)
        hash_part(hasher, sha256_bytes(path.read_bytes()))
    return hasher.hexdigest()


def config_data(root: Path, project_path: Path) -> dict[str, Any]:
    if project_path.name == "pyrightconfig.json":
        return json.loads(project_path.read_text(encoding="utf-8"))
    if project_path.name == "pyproject.toml":
        try:
            import tomllib
        except ImportError:
            return {}
        try:
            data = tomllib.loads(project_path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError:
            return {}
        return data.get("tool", {}).get("pyright", {})
    return {}


def choose_project(root: Path, requested: str | None) -> Path | None:
    if requested:
        project = (root / requested).resolve()
    else:
        candidates = [root / "pyrightconfig.json", root / "pyproject.toml"]
        project = next((candidate.resolve() for candidate in candidates if candidate.is_file()), None)
        if project is None:
            return None
    if project and not project.is_relative_to(root):
        raise ValueError(f"Python project file must be under root: {project}")
    if project and not project.is_file():
        raise FileNotFoundError(f"Python project file does not exist: {project}")
    return project


def line_starts(source: bytes) -> list[int]:
    starts = [0]
    for index, byte in enumerate(source):
        if byte == 10:
            starts.append(index + 1)
    return starts


def line_for_byte(starts: list[int], offset: int) -> int:
    return bisect_right(starts, offset)


def byte_offset(starts: list[int], line: int, column: int) -> int:
    return starts[line - 1] + column


def source_range(starts: list[int], node: ast.AST) -> dict[str, int]:
    start_line = getattr(node, "lineno")
    end_line = getattr(node, "end_lineno", start_line)
    start_column = getattr(node, "col_offset")
    end_column = getattr(node, "end_col_offset", start_column)
    return {
        "start_byte": byte_offset(starts, start_line, start_column),
        "end_byte": byte_offset(starts, end_line, end_column),
        "start_line": start_line,
        "end_line": end_line,
        "start_column": start_column,
        "end_column": end_column,
    }


def symbol_range(root: Path, path: Path, starts: list[int], node: ast.AST) -> dict[str, Any]:
    range_data = source_range(starts, node)
    return {
        "path": normalized_relative(root, path),
        "start_byte": range_data["start_byte"],
        "end_byte": range_data["end_byte"],
    }


def utf16_character(source: str, starts: list[int], line: int, byte_column: int) -> int:
    encoded = source.encode()
    line_start = starts[line]
    prefix = encoded[line_start : line_start + byte_column].decode("utf-8")
    return len(prefix.encode("utf-16-le")) // 2


def lsp_position(source: str, starts: list[int], line: int, byte_column: int) -> dict[str, int]:
    return {"line": line - 1, "character": utf16_character(source, starts, line - 1, byte_column)}


def definition_position(source: str, starts: list[int], node: ast.AST) -> dict[str, int]:
    if isinstance(node, ast.Attribute):
        line = getattr(node, "end_lineno", node.lineno)
        end_column = getattr(node, "end_col_offset", node.col_offset)
        byte_column = end_column - len(node.attr.encode())
    else:
        line = node.lineno
        byte_column = node.col_offset
    return lsp_position(source, starts, line, byte_column)


def protocol_method_lines(
    trees: dict[str, tuple[ast.AST, str, bytes, list[int]]],
) -> set[tuple[str, int]]:
    lines: set[tuple[str, int]] = set()
    for relative_path, (tree, _, _, _) in trees.items():
        for node in ast.walk(tree):
            if not isinstance(node, ast.ClassDef):
                continue
            is_protocol = any(
                (isinstance(base, ast.Name) and base.id == "Protocol")
                or (isinstance(base, ast.Attribute) and base.attr == "Protocol")
                for base in node.bases
            )
            if not is_protocol:
                continue
            for member in node.body:
                if isinstance(member, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    lines.add((relative_path, member.lineno))
    return lines


def byte_column_from_utf16(line_text: str, character: int) -> int:
    units = 0
    for index, char in enumerate(line_text):
        next_units = units + (2 if ord(char) > 0xFFFF else 1)
        if next_units > character:
            return len(line_text[:index].encode())
        units = next_units
        if units == character:
            return len(line_text[: index + 1].encode())
    return len(line_text.encode())


def location_symbol(root: Path, sources: dict[str, tuple[Path, bytes, list[int]]], location: dict[str, Any]) -> dict[str, Any] | None:
    uri = location.get("uri") or location.get("targetUri")
    location_range = location.get("range") or location.get("targetSelectionRange")
    if not uri or not location_range:
        return None
    path = path_from_uri(uri)
    try:
        relative_path = normalized_relative(root, path)
    except ValueError:
        return None
    source_entry = sources.get(relative_path)
    if source_entry is None:
        return None
    source_path, source, starts = source_entry
    start = location_range["start"]
    end = location_range.get("end", start)
    lines = source.decode().splitlines(keepends=True)
    if not (0 <= start["line"] < len(lines) and 0 <= end["line"] < len(lines)):
        return None
    start_byte = starts[start["line"]] + byte_column_from_utf16(lines[start["line"]], start["character"])
    end_byte = starts[end["line"]] + byte_column_from_utf16(lines[end["line"]], end["character"])
    if start_byte >= end_byte:
        return None
    return {"path": relative_path, "start_byte": start_byte, "end_byte": end_byte}


def target_name(source: str, node: ast.Call) -> str:
    segment = ast.get_source_segment(source, node.func) or ""
    match = TARGET_NAME_RE.search(segment)
    return match.group(1) if match else segment.strip() or "<call>"


def hover_text(result: Any) -> str:
    if not isinstance(result, dict):
        return ""
    contents = result.get("contents", [])
    if isinstance(contents, str):
        return contents
    if isinstance(contents, dict):
        return str(contents.get("value", ""))
    if isinstance(contents, list):
        values = []
        for item in contents:
            if isinstance(item, str):
                values.append(item)
            elif isinstance(item, dict):
                values.append(str(item.get("value", "")))
        return "\n".join(values)
    return ""


class CallCollector(ast.NodeVisitor):
    def __init__(
        self,
        root: Path,
        path: Path,
        source: str,
        source_bytes: bytes,
        rpc: JsonRpc,
        sources,
        protocol_methods: set[tuple[str, int]],
    ) -> None:
        self.root = root
        self.path = path
        self.source = source
        self.source_bytes = source_bytes
        self.starts = line_starts(source_bytes)
        self.rpc = rpc
        self.sources = sources
        self.protocol_methods = protocol_methods
        self.callers: list[ast.AST] = []
        self.calls: list[dict[str, Any]] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.callers.append(node)
        self.generic_visit(node)
        self.callers.pop()

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.callers.append(node)
        self.generic_visit(node)
        self.callers.pop()

    def visit_Call(self, node: ast.Call) -> None:
        if self.callers:
            self.calls.append(self.fact(node, self.callers[-1]))
        self.generic_visit(node)

    def fact(self, node: ast.Call, caller: ast.AST) -> dict[str, Any]:
        call_range = source_range(self.starts, node)
        caller_symbol = symbol_range(self.root, self.path, self.starts, caller)
        position = definition_position(self.source, self.starts, node.func)
        response = self.rpc.request(
            "textDocument/definition",
            {
                "textDocument": {"uri": uri_for(self.path)},
                "position": position,
            },
        )
        hover = self.rpc.request(
            "textDocument/hover",
            {
                "textDocument": {"uri": uri_for(self.path)},
                "position": position,
            },
        )
        hover_value = hover_text(hover)
        locations = response if isinstance(response, list) else ([] if not response else [response])
        symbols = []
        external = False
        for location in locations:
            symbol = location_symbol(self.root, self.sources, location)
            if symbol is not None:
                symbols.append(symbol)
            else:
                external = True
        unique: dict[tuple[str, int, int], dict[str, Any]] = {}
        for symbol in symbols:
            unique[(symbol["path"], symbol["start_byte"], symbol["end_byte"])] = symbol
        targets = [unique[key] for key in sorted(unique)]
        target_is_protocol = False
        if len(targets) == 1:
            target_source = self.sources.get(targets[0]["path"])
            if target_source is not None:
                target_is_protocol = (
                    targets[0]["path"],
                    line_for_byte(target_source[2], targets[0]["start_byte"]),
                ) in self.protocol_methods
        hover_first_line = hover_value.splitlines()[0] if hover_value else ""
        is_unknown = bool(
            re.search(
                r"^\((?:variable|parameter|property|unknown)\).*\b(?:Any|Unknown)\b",
                hover_first_line,
            )
        )
        is_indirect = bool(
            re.search(r"^\((?:variable|parameter|property|type alias)\)", hover_value)
        )
        is_possible = bool(
            re.search(
                r"^\((?:method|variable|parameter|property)\).*\b(?:Protocol|overload)\b",
                hover_first_line,
            )
        )
        if is_unknown:
            resolution = "unresolved"
            target = None
            possible_targets = []
        elif is_indirect:
            resolution = "function-value"
            target = None
            possible_targets = []
        elif len(targets) == 1 and (is_possible or target_is_protocol):
            resolution = "possible"
            target = None
            possible_targets = targets
        elif len(targets) == 1:
            resolution = "static"
            target = targets[0]
            possible_targets = []
        elif len(targets) > 1:
            resolution = "ambiguous"
            target = None
            possible_targets = targets
        elif external:
            resolution = "external"
            target = None
            possible_targets = []
        else:
            resolution = "unresolved"
            target = None
            possible_targets = []
        return {
            "path": normalized_relative(self.root, self.path),
            **call_range,
            "caller": caller_symbol,
            "target_name": target_name(self.source, node),
            "target": target,
            "possible_targets": possible_targets,
            "resolution": resolution,
        }


def load_source(root: Path, path: Path) -> tuple[ast.AST, str, bytes, list[int]]:
    data = path.read_bytes()
    text = data.decode("utf-8")
    tree = ast.parse(text, filename=str(path))
    return tree, text, data, line_starts(data)


def initialize_rpc(rpc: JsonRpc, root: Path, sources: dict[str, tuple[Path, bytes, list[int]]]) -> None:
    rpc.request(
        "initialize",
        {
            "processId": os.getpid(),
            "rootUri": uri_for(root),
            "workspaceFolders": [{"uri": uri_for(root), "name": root.name}],
            "capabilities": {
                "textDocument": {
                    "definition": {"linkSupport": True},
                    "hover": {"contentFormat": ["plaintext"]},
                }
            },
            "clientInfo": {"name": "zvec-grep-python-callfacts", "version": "1"},
        },
    )
    rpc.notify("initialized", {})
    rpc.notify("workspace/didChangeConfiguration", {"settings": {}})
    for relative_path, (path, data, _) in sorted(sources.items()):
        rpc.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri_for(path),
                    "languageId": "python",
                    "version": 1,
                    "text": data.decode("utf-8"),
                }
            },
        )


def build_context(
    root: Path,
    project: Path | None,
    tool_root: Path,
    context_file_list: list[dict[str, str]],
) -> dict[str, Any]:
    settings: dict[str, str] = {}
    config = config_data(root, project) if project else {}
    for key in sorted(config):
        settings[key] = stable_json(config[key])
    target = str(config.get("pythonVersion", "default"))
    return {
        "pyright_version": pyright_version(tool_root),
        "python_version": platform.python_version(),
        "target_version": target,
        "project_path": normalized_relative(root, project) if project else "",
        "typeshed_sha256": typeshed_digest(tool_root),
        "settings": settings,
        "context_files": context_file_list,
    }


def generate(root: Path, project: str | None, output: str | None) -> dict[str, Any]:
    root = root.resolve()
    if not root.is_dir():
        raise NotADirectoryError(root)
    tool_root = Path(__file__).resolve().parent
    paths = source_paths(root)
    source_data: dict[str, tuple[Path, bytes, list[int]]] = {}
    trees: dict[str, tuple[ast.AST, str, bytes, list[int]]] = {}
    for path in paths:
        relative_path = normalized_relative(root, path)
        tree, text, data, starts = load_source(root, path)
        source_data[relative_path] = (path, data, starts)
        trees[relative_path] = (tree, text, data, starts)
    project_path = choose_project(root, project)
    protocol_methods = protocol_method_lines(trees)
    context = build_context(root, project_path, tool_root, hashed_files(root, context_paths(root)))
    command = pyright_command(tool_root)
    rpc = JsonRpc(command, root)
    calls: list[dict[str, Any]] = []
    try:
        initialize_rpc(rpc, root, source_data)
        for relative_path in sorted(trees):
            tree, text, data, _ = trees[relative_path]
            collector = CallCollector(
                root,
                source_data[relative_path][0],
                text,
                data,
                rpc,
                source_data,
                protocol_methods,
            )
            collector.visit(tree)
            calls.extend(collector.calls)
    finally:
        rpc.close()
    calls.sort(key=lambda call: (call["path"], call["start_byte"], call["end_byte"]))
    artifact = {
        "schema": SCHEMA,
        "version": VERSION,
        "context": context,
        "context_sha256": context_fingerprint(context),
        "files": hashed_files(root, paths),
        "calls": calls,
    }
    output_path = (root / output).resolve() if output else root / ".zvec-grep" / OUTPUT_FILE
    if not output_path.is_relative_to(root):
        raise ValueError(f"output must be under root: {output_path}")
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=output_path.parent, prefix=f".{output_path.name}.", delete=False
    ) as temporary:
        temporary.write(json.dumps(artifact, separators=(",", ":")) + "\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(output_path)
    return {
        "output": output_path,
        "files": len(paths),
        "calls": len(calls),
        "context_sha256": artifact["context_sha256"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--project", help="relative pyrightconfig.json or pyproject.toml")
    parser.add_argument("--output", help="relative output path")
    arguments = parser.parse_args()
    try:
        result = generate(arguments.root, arguments.project, arguments.output)
    except (LspError, OSError, SyntaxError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"python-callfacts: {error}", file=sys.stderr)
        return 1
    print(
        f"Python callfacts: {result['calls']} calls, {result['files']} files -> "
        f"{result['output']} ({result['context_sha256']})",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
