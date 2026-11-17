#!/usr/bin/env bash
set -euo pipefail

# This producer intentionally uses rustc_driver rather than Graphify, SCIP, or
# rust-analyzer. It is a pinned compiler-internal tool: rustc and rustc-dev must
# come from the same rustup toolchain.

ROOT=""
MANIFEST=""
TOOLCHAIN="${ZG_RUST_TOOLCHAIN:-1.98.0}"
TARGET="${CARGO_BUILD_TARGET:-}"
ALL_TARGETS=1
EXTRA_CARGO_ARGS=()

while (($# > 0)); do
    case "$1" in
        --root)
            ROOT=$2
            shift 2
            ;;
        --manifest-path)
            MANIFEST=$2
            shift 2
            ;;
        --toolchain)
            TOOLCHAIN=$2
            shift 2
            ;;
        --target)
            TARGET=$2
            shift 2
            ;;
        --no-all-targets)
            ALL_TARGETS=0
            shift
            ;;
        --)
            shift
            EXTRA_CARGO_ARGS+=("$@")
            break
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [[ -z "$ROOT" || -z "$MANIFEST" ]]; then
    echo "usage: $0 --root ROOT --manifest-path CARGO_MANIFEST [--toolchain TOOLCHAIN] [--target TARGET] [--no-all-targets] [-- CARGO_ARGS...]" >&2
    exit 2
fi

ROOT=$(cd "$ROOT" && pwd -P)
MANIFEST=$(cd "$(dirname "$MANIFEST")" && pwd -P)/$(basename "$MANIFEST")
if [[ ! -f "$MANIFEST" ]]; then
    echo "manifest does not exist: $MANIFEST" >&2
    exit 2
fi
if [[ "$(basename "$MANIFEST")" != "Cargo.toml" ]]; then
    echo "manifest must be named Cargo.toml: $MANIFEST" >&2
    exit 2
fi
case "$MANIFEST" in
    "$ROOT"/*) ;;
    *)
        echo "manifest must be under the analysis root: $MANIFEST" >&2
        exit 2
        ;;
esac

if ! rustup component list --toolchain "$TOOLCHAIN" | grep -q '^rustc-dev-.* (installed)$'; then
    echo "matching rustc-dev is required for toolchain $TOOLCHAIN" >&2
    echo "install with: rustup component add rustc-dev --toolchain $TOOLCHAIN" >&2
    exit 2
fi

SYSROOT=$(rustup run "$TOOLCHAIN" rustc --print sysroot)
HOST=$(rustup run "$TOOLCHAIN" rustc -vV | awk '/^host: / {print $2}')
RUSTC_VERSION=$(rustup run "$TOOLCHAIN" rustc -vV | sed -n '1p')
RUSTC_COMMIT=$(rustup run "$TOOLCHAIN" rustc -vV | awk '/^commit-hash: / {print $2}')
if [[ -z "$TARGET" ]]; then
    TARGET=$HOST
fi

PRIVATE_LIB="$SYSROOT/lib/rustlib/$HOST/lib"
NATIVE_LIB="$SYSROOT/lib"
DRIVER_LIB=$(find "$NATIVE_LIB" -maxdepth 1 -name 'librustc_driver-*' -type f -print -quit)
if [[ -z "$DRIVER_LIB" ]]; then
    echo "rustc_driver library not found under $NATIVE_LIB" >&2
    exit 2
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/zg-rust-callfacts.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/rust-callfacts"
RUN_DIR="$WORK/fragments"
TARGET_DIR="$WORK/cargo-target"
mkdir -p "$RUN_DIR"

RUSTC_BOOTSTRAP=1 rustup run "$TOOLCHAIN" rustc \
    --edition=2024 \
    --crate-name rust_callfacts \
    --crate-type=bin \
    -L "dependency=$PRIVATE_LIB" \
    -L "native=$NATIVE_LIB" \
    --extern "rustc_driver=$DRIVER_LIB" \
    -C "link-arg=-Wl,-rpath,$NATIVE_LIB" \
    -C "link-arg=-Wl,-rpath,$PRIVATE_LIB" \
    -o "$BIN" \
    "$(dirname "$0")/src/main.rs"

LOCKFILE=""
if [[ -f "$(dirname "$MANIFEST")/Cargo.lock" ]]; then
    LOCKFILE="$(dirname "$MANIFEST")/Cargo.lock"
fi
TOOLCHAIN_FILE=""
for candidate in "$(dirname "$MANIFEST")/rust-toolchain.toml" "$(dirname "$MANIFEST")/rust-toolchain" "$ROOT/rust/rust-toolchain.toml" "$ROOT/rust/rust-toolchain" "$ROOT/rust-toolchain.toml" "$ROOT/rust-toolchain"; do
    if [[ -f "$candidate" ]]; then
        TOOLCHAIN_FILE=$candidate
        break
    fi
done

export ZG_RUST_CALLFACTS_ROOT="$ROOT"
export ZG_RUST_CALLFACTS_RUN_DIR="$RUN_DIR"
export ZG_RUST_CALLFACTS_SYSROOT="$SYSROOT"
export RUSTC_BOOTSTRAP=1

if ((ALL_TARGETS)); then
    TARGET_ARGS=(--all-targets)
else
    TARGET_ARGS=()
fi
if [[ -n "$TARGET" ]]; then
    TARGET_ARGS+=(--target "$TARGET")
fi

env -u RUSTC_WRAPPER \
    RUSTC="$BIN" \
    CARGO_TARGET_DIR="$TARGET_DIR" \
    rustup run "$TOOLCHAIN" cargo check \
    --manifest-path "$MANIFEST" \
    --workspace \
    "${TARGET_ARGS[@]}" \
    "${EXTRA_CARGO_ARGS[@]}"

OUTPUT="$ROOT/.zvec-grep/rust-callfacts-v1.json"
ZG_RUST_CALLFACTS_MANIFEST="$MANIFEST" \
ZG_RUST_CALLFACTS_LOCKFILE="$LOCKFILE" \
ZG_RUST_CALLFACTS_TOOLCHAIN="$TOOLCHAIN_FILE" \
ZG_RUST_CALLFACTS_RUSTC_VERSION="$RUSTC_VERSION" \
ZG_RUST_CALLFACTS_RUSTC_COMMIT="$RUSTC_COMMIT" \
ZG_RUST_CALLFACTS_HOST="$HOST" \
ZG_RUST_CALLFACTS_TARGET="$TARGET" \
ZG_RUST_CALLFACTS_EDITION="2024" \
    "$BIN" --merge --root "$ROOT" --run-dir "$RUN_DIR" --output "$OUTPUT"

echo "Rust call facts: $OUTPUT"
