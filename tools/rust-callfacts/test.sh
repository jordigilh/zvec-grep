#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/zg-rust-callfacts-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/src"
cat > "$WORK/Cargo.toml" <<'EOF'
[package]
name = "callfacts-smoke"
version = "0.1.0"
edition = "2024"
EOF
cat > "$WORK/src/lib.rs" <<'EOF'
pub fn helper(value: i32) -> i32 {
    value
}

pub fn caller(value: i32) -> i32 {
    helper(value)
}
EOF

"$ROOT/tools/rust-callfacts/generate.sh" \
    --root "$WORK" \
    --manifest-path "$WORK/Cargo.toml" \
    --no-all-targets

python3 - "$WORK/.zvec-grep/rust-callfacts-v1.json" <<'PY'
import json
import sys
from pathlib import Path

artifact = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert artifact["schema"] == "zvec-grep.rust-callfacts"
assert artifact["version"] == 1
assert any(call["resolution"] == "static" for call in artifact["calls"])
print("Rust callfacts producer smoke: ok")
PY
