#!/usr/bin/env python3
"""Focused producer checks for the pinned Python call-facts tool."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> int:
    tool = Path(__file__).with_name("generate.py")
    with tempfile.TemporaryDirectory(prefix="zg-python-callfacts-test-") as temporary:
        root = Path(temporary)
        (root / "src").mkdir()
        (root / "pyrightconfig.json").write_text(
            json.dumps({"include": ["src"], "pythonVersion": "3.13"}),
            encoding="utf-8",
        )
        source = "\n".join(
            [
                "from typing import Any, Protocol",
                "import math",
                "",
                "class Worker(Protocol):",
                "    def work(self, value: int) -> int: ...",
                "",
                "def protocol_caller(worker: Worker) -> int:",
                "    return worker.work(1)",
                "",
                "def helper(value: str) -> str:",
                "    return value",
                "",
                "def caller() -> str:",
                '    note = "é 😀"',
                "    return helper(note)",
                "",
                "def alias() -> str:",
                "    fn = helper",
                '    return fn("alias")',
                "",
                "def dynamic() -> str:",
                "    fn: Any = helper",
                '    return fn("dynamic")',
                "",
                "def external() -> float:",
                "    return math.sqrt(4)",
                "",
            ]
        )
        source_path = root / "src" / "fixture.py"
        source_path.write_text(source, encoding="utf-8")

        command = [sys.executable, str(tool), "--root", str(root)]
        subprocess.run(command, check=True, capture_output=True, text=True)
        output = root / ".zvec-grep" / "python-callfacts-v1.json"
        first = output.read_bytes()
        artifact = json.loads(first)
        calls = artifact["calls"]
        resolutions = {call["resolution"] for call in calls}
        assert artifact["schema"] == "zvec-grep.python-callfacts"
        assert artifact["version"] == 1
        assert artifact["context"]["typeshed_sha256"] != "unknown"
        assert {"static", "possible", "function-value", "unresolved", "external"} <= resolutions
        source_bytes = source_path.read_bytes()
        for call in calls:
            assert call["target_name"].encode() in source_bytes[
                call["start_byte"] : call["end_byte"]
            ]
            assert call["caller"]["start_byte"] <= call["start_byte"]
            assert call["end_byte"] <= call["caller"]["end_byte"]

        subprocess.run(command, check=True, capture_output=True, text=True)
        assert output.read_bytes() == first
    print("Python callfacts producer tests: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
