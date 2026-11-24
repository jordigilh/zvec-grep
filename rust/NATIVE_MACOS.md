# Native macOS distribution (issue #75)

`zg` runs as a native macOS arm64 process — no Podman guest, no 7 GB VM memory
ceiling, and a path to Apple-silicon GPU acceleration (#74). Linux and CI stay
containerized.

## Why native

Measured on this host (2026-11-24), same corpus, same model (jina q8):

| | Native | Container (7 GB Podman VM) |
|---|---|---|
| 19-file fixture full index | 34.5 s, 6.31 GB peak RSS | **OOM-killed** (twice; ~5.5 GB available in guest) |

The Podman `vfkit` machine has no GPU device and Apple GPUs have no Linux
driver, so the container can never reach Metal/ANE regardless of
configuration. The guest also retains peak page footprint after spikes until
the VM is restarted.

## Build (CLT only, no Xcode.app)

```sh
S=/Library/Developer/CommandLineTools/SDKs/MacOSX27.2.sdk
env -u DEVELOPER_DIR SDKROOT=$S CPATH=$S/usr/include \
  BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$S -isysroot $S -I$S/usr/include -I$S/usr/include/c++/v1" \
  CFLAGS="-isysroot $S -I$S/usr/include/c++/v1" \
  CXXFLAGS="-isysroot $S -I$S/usr/include/c++/v1" \
  cargo build --release -p zg
```

~3 min on Apple silicon. ONNX Runtime is statically linked (ort crate
`download-binaries`), so the release artifact needs no `libonnxruntime.dylib`;
swap to `load-dynamic` if a shared ORT is ever required.

## Prefix layout

```
<prefix>/bin/zg                          # carries @executable_path/../lib rpaths
<prefix>/lib/libzvec_c_api.dylib
<prefix>/lib/data/jieba_dict/{jieba.dict.utf8,hmm_model.utf8}
<prefix>/home/                           # ZVEC_GREP_HOME (daemon state)
<prefix>/logs/                           # launchd stdout/stderr
```

The jieba dictionaries are copied from the `zvec-rust-sys` build output
(`target/release/build/zvec-rust-sys-*/out/zvec-prebuilt/data/jieba_dict/`).
The FTS tokenizer resolves them relative to the binary; a missing dict changes
tokenization silently, so the layout check is part of verification below.

The model cache is shared read-only with the previous deployment
(`ZVEC_GREP_MODEL_CACHE=~/.engram/zvec-grep/model`).

## launchd

Template: [`native-macos-launchd.plist`](../container/native-macos-launchd.plist).
Install:

```sh
cp container/native-macos-launchd.plist ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
# edit prefix paths if not ~/.local/zvec-grep-native
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
```

Runs `zg --server run --listen 127.0.0.1:7999 --mcp-toolset full` — the same
endpoint and toolset as the container, so the Engram gateway and OpenCode
plugin need no change. The loopback-proxy socat container is no longer needed.

## Install verification

1. `curl http://127.0.0.1:7999/healthz` → `{"status":"ok"}`
2. MCP `tools/list` → 16 tools, identical names to the container `full` toolset
3. One FTS search (proves jieba dict resolution; a CJK query exercises the
   segmenter) and one vector-route search (proves the ORT embedding path)
4. Quality lanes: all nine fixtures pass `check_baselines_multilanguage.py`
   against the floors pinned from the release binary

## Upgrade

1. Build, stage a new prefix (or replace `bin/zg` in place)
2. `launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep-native`
3. `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist`
4. Re-run install verification. If the ORT build changed, expect ranking drift
   (see below) and force-reindex active workspaces.

## Rollback to the container

The container LaunchAgent plist and images are kept. Rollback:

```sh
launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep-native
podman machine start   # if the VM is down
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep.plist
```

Note: container-built `.zvec-grep` indexes fail to open in the native build
(and vice versa), so after any direction switch, active workspaces must be
re-indexed (`zvec_grep_index` with `rebuild: true`).

## Cross-runtime embedding drift (important)

ONNX Runtime **builds** — not just versions — drift embeddings. Measured:

- native (static crate ORT) vs container ORT: 2 of 8 parity-probe queries
  reorder, including one #1 flip;
- the previous lane floors were pinned by a binary (sha `eca0565e…`, commit
  `8a2cd36`) whose metrics cannot be reproduced by rebuilding its own commit
  with the CLT toolchain: identical, deterministic, but different numbers.

Consequences, applied in this change:

- Lane floors are **re-pinned from the release binary** (see
  `benchmarks/quality-lanes/baselines-multilanguage.json` provenance). This is
  the reviewed re-pin the lane policy requires; the old floors referenced a
  binary that no longer exists.
- Every runtime switch must be followed by a rebuild of active workspace
  indexes (the native server refuses to open container-built storage, which
  enforces this rather than silently mixing).
- Tracking issue: record `backend` + `dtype` per generation in the manifest and
  refuse cross-backend queries (#75 scope item 3). Until that lands, backend
  identity is implied by which binary built the index.

## Open items

- Engine-level workspace-roots allowlist: the container wrapper enforced
  `ZVEC_GREP_WORKSPACE_ROOTS`; the native server trusts loopback callers the
  same way the container's MCP endpoint did. Needs an engine-level guard before
  any non-loopback exposure (#75 scope item 4).
- GPU indexing backend (#74), batching/thread fixes (#73), worktree index
  seeding (#72).
