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
`download-binaries`), so this deployment needs no `libonnxruntime.dylib`. The
release-artifact tarball (#75 scope item 1) still ships one if the
`load-dynamic` linkage is ever adopted; that packaging decision is tracked in
#75 and not part of this change.

## Install

Set the prefix and model cache for your user (the examples below assume
`PREFIX=$HOME/.local/zvec-grep-native` and a shared model cache at
`$HOME/.engram/zvec-grep/model`; adjust to taste):

```sh
PREFIX=$HOME/.local/zvec-grep-native
MODEL_CACHE=$HOME/.engram/zvec-grep/model   # must contain the per-language models
```

Create the prefix layout and populate it from the build output:

```sh
mkdir -p "$PREFIX/bin" "$PREFIX/lib/data/jieba_dict" "$PREFIX/home" "$PREFIX/logs"
cp rust/target/release/zg "$PREFIX/bin/"
cp rust/target/release/libzvec_c_api.dylib "$PREFIX/lib/"
D=$(find rust/target/release/build -path '*zvec-prebuilt/data/jieba_dict' | head -1)
cp "$D"/* "$PREFIX/lib/data/jieba_dict/"
```

The binary carries `@executable_path/../lib` rpaths, so it finds the dylib at
this layout. The jieba dictionaries are mandatory: the FTS tokenizer resolves
them relative to the binary, and a missing dict changes tokenization silently.

Generate the LaunchAgent plist from the template (launchd does not expand
variables, so user-specific paths are substituted at install time):

```sh
mkdir -p "$HOME/Library/LaunchAgents"
sed -e "s|@HOME@|$HOME|g" -e "s|@PREFIX@|$PREFIX|g" \
    -e "s|@MODEL_CACHE@|$MODEL_CACHE|g" -e "s|@PORT@|7998|g" \
    container/native-macos-launchd.plist.template \
    > ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
```

## Migration from the container deployment (staged)

Do **not** point the native service at port 7999 while the container still
serves it — the bind would fail (or, if the container is momentarily down, the
still-installed container unit could answer verification requests and conceal
a failed migration). Stage on 7998 first, as above (`@PORT@` → 7998).

1. Verify the staged native service (see "Install verification") on **7998**.
2. Stop the container deployment:
   ```sh
   launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep   # container LaunchAgent
   podman stop engram-zvec-grep engram-zvec-grep-proxy 2>/dev/null || true
   ```
3. Gate the switch on the port actually being free. A TCP **listener** check
   blocks the switch if anything still serves 7999 — unlike a `curl` timeout,
   which can silently report "free":
   ```sh
   if lsof -nP -iTCP:7999 -sTCP:LISTEN >/dev/null 2>&1; then
     echo "7999 is still held by a listener; aborting the switch" >&2
     exit 1
   fi
   ```
4. Switch the native service to 7999:
   ```sh
   launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep-native
   sed -i '' 's|127.0.0.1:7998|127.0.0.1:7999|' \
     ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
   ```
5. Re-run the verification below against 7999.
6. Re-index active workspaces (see "Index compatibility" — storage written by
   the container build does not open natively).

The container LaunchAgent plist and images are kept for rollback. The socat
loopback-proxy container is retired.

## Install verification

Run these against the port the service is **currently** listening on — during
staged migration that is 7998, after the switch 7999:

```sh
PORT=7998   # or 7999 once the native service owns the endpoint
curl -sf "http://127.0.0.1:$PORT/healthz" | grep -q '"status":"ok"' \
  || { echo "health check failed on $PORT"; exit 1; }
```

Then, over MCP on the same port:

1. MCP `tools/list` → 16 tools, identical names to the container `full` toolset
2. One FTS search (proves jieba dict resolution; a CJK query exercises the
   segmenter) and one vector-route search (proves the ORT embedding path)
3. Quality lanes: all nine fixtures pass `check_baselines_multilanguage.py`
   against the floors pinned from the release binary

## Upgrade

1. Build the new release and stage it in a **new** prefix (or replace the
   contents of the existing one while the service is stopped — do not
   overwrite a running binary).
2. Stop the service:
   ```sh
   launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep-native
   ```
3. Point the service at the staged release **without moving state**. The
   state directory (`ZVEC_GREP_HOME`), model cache, and port must survive the
   upgrade. Either edit the installed plist in place — change only the
   executable path in `ProgramArguments` — or, if regenerating from the
   template, pass the **existing** values explicitly:
   ```sh
   # carry these over from the installed plist; do not derive them from the
   # new prefix:
   STATE=$HOME/.local/zvec-grep-native/home     # previous ZVEC_GREP_HOME
   MODEL_CACHE=$HOME/.engram/zvec-grep/model    # previous model cache
   PORT=7999                                    # the service port in production
   NEW_PREFIX=$HOME/.local/zvec-grep-native-<version>

   sed -e "s|@HOME@|$HOME|g" -e "s|@PREFIX@|$NEW_PREFIX|g" \
       -e "s|@MODEL_CACHE@|$MODEL_CACHE|g" -e "s|@PORT@|$PORT|g" \
       container/native-macos-launchd.plist.template \
       > ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
   # then edit ZVEC_GREP_HOME in the generated plist to $STATE:
   sed -i '' "s|<string>$NEW_PREFIX/home</string>|<string>$STATE</string>|" \
       ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
   ```
   Note the template's `@PREFIX@/home` default is wrong for upgrades — the
   state directory is always the previous install's `ZVEC_GREP_HOME`, unless
   you deliberately want a fresh start.
4. Start the service:
   ```sh
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep-native.plist
   ```
5. Re-run install verification (against `$PORT` above). If the ORT build
   changed, expect ranking drift (see below) and re-index active workspaces.

## Rollback to the container

```sh
launchctl bootout gui/$(id -u)/io.vectorize.zvec-grep-native
podman machine start   # if the VM is down
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.vectorize.zvec-grep.plist
```

## Index compatibility across runtimes

Index storage written by the container build (Linux zvec) failed to open in
the native build during migration — observed as
`ZG.ENGINE.STORAGE_FAILURE: zvec open collection … Failed to open index` on
every container-built workspace. **This is an empirical observation about
cross-platform storage, not a backend-identity guarantee**: it can leave
storage that no runtime can open, and `zvec_grep_index` with `rebuild: true`
does not recover from unreadable cross-build storage on its own — it fails
with `STORAGE_FAILURE`; clearing `.zvec-grep` and indexing fresh is the
documented recovery:

```sh
# per workspace; if rebuild fails to open the old storage, clear it first
rm -rf <root>/.zvec-grep
# then index via MCP zvec_grep_index {"root": <root>} or `zg --index <root>`
```

Beyond that hard failure, the index load path now identifies the embedding
runtime: every generation records a `backend` + `dtype` identity in its
manifest — the transformers backend label names the provider **and the
linked ONNX Runtime version** (e.g. `onnx-cpu/ort-1.22.0`, #94; llama.cpp
labels stay version-less) — and a serving runtime
reporting a different pair refuses (see the next section). Storage that
*does* open is no longer an implicit compatibility guarantee — the identity
check is the guarantee.

## One-time fleet reindex after backend identity (issue #85)

Since #78 (merged `72943ce`) + #81 (merged `89ab30a`), every index generation
records the identity of the embedding runtime that produced its vectors
(`backend` + `dtype`, e.g. `onnx-cpu/ort-1.22.0` / `q8`), and vector search
plus incremental indexing fail closed when the serving runtime's identity
does not match. Indexes built
before this recording have no identity, and **every such index was by
definition built by a runtime older than the current binary** — so after
upgrading the daemon, all pre-existing **local-model** workspaces refuse
vector search and incremental indexing until they are rebuilt once. Remote-model
(e.g. qwen) indexes are unaffected: remote backends record no identity, and
"no identity" on both sides matches.

#94 (merged `8e46c49`, after this runbook was first drafted) tightened the
label further: the transformers backend reads the linked ONNX Runtime
version live (via the quarantined `zg-ort-version` crate) and records it in
the backend label as `onnx-<provider>/ort-<version>`. An ORT version change
is therefore itself a reindex trigger — a daemon upgraded across ORT
versions refuses indexes rebuilt against the older one. This was deliberate
scoping: #86 shipped inside this #85 reindex window, so the one-time fleet
reindex below covers the identity-less legacy indexes, any index whose
recorded ORT version differs from the new binary's, and any index rebuilt by
a pre-#94 binary (which records `onnx-cpu` with no version — the same
mismatch the rollback section describes).

The refusal is terminal, not retryable: over MCP the error renders as
`ZG.ENGINE.INDEX_BACKEND_MISMATCH` with `retryable: false`, naming both
identities and the recovery. Two message shapes exist (same code, same
remedy); both share the prefix "workspace index embedding backend identity
does not match this runtime", which is the reliable grep anchor:

- at open time: "workspace index embedding backend identity does not match
  this runtime: index has no recorded identity (built before backend
  recording), this runtime has backend=… dtype=…; rebuild the index with
  `zg --index --rebuild`"
- mid-run: "… the storage opened with …, but the runtime produced vectors
  with …; rebuild the index with `zg --index --rebuild`"

### Runbook

1. **Restart the daemon** onto the new binary (see "Upgrade" above). Until
   restarted it keeps serving the old behavior — there is no rolling restart.
   Verify with the install-verification block against the service port.
2. **Enumerate registered workspaces** from the registry,
   `~/.zvec-grep/workspaces.json`:

   ```sh
   jq -r '.workspaces[] | "\(.name)\t\(.root)"' ~/.zvec-grep/workspaces.json
   ```

   The registry is `{version, workspaces: [{name, root}]}`. Read it only —
   both structs are `deny_unknown_fields`, so hand-editing it (e.g. adding a
   comment field) makes it unreadable. Identify local-model workspaces by
   checking each manifest's embedding `provider == "local"`. Note that
   `zg status` / `zvec_grep_status` will **not** flag these workspaces ahead
   of time: their `rebuild_required` state comes from index-version mismatch
   or manifest decode failure only, and a legacy identity-less manifest is
   version 3 and decodes cleanly — it reports Ready until the first vector
   query or incremental update refuses. The manifest provider check (or the
   first refusal message itself) is the discovery mechanism.
3. **Rebuild each affected workspace**:

   ```sh
   zg --index <root> --rebuild
   ```

   If the rebuild fails with `STORAGE_FAILURE` (unreadable cross-build
   storage), clear `<root>/.zvec-grep` first and index fresh. For a
   user-owned rather than centrally managed fleet, no enumeration is needed:
   the refusal message itself points each user at `zg --index --rebuild`.
   Note the rebuild records the **live** runtime identity (actual execution
   path, not the configured device): a rebuild that fell back to CPU relabels
   the generation accordingly, and a later machine that does get the
   accelerated provider will be refused again, by design, and needs its own
   rebuild.
4. **During the window, FTS keeps working.** The outage is vector-search-only
   per workspace: non-vector requests never acquire an embedding model, so
   they never hit the identity check, and a failed auto-refresh whose error
   is `INDEX_BACKEND_MISMATCH` still serves the stored full-text results
   (warn-logged). No action is needed to keep exact/FTS routes alive.

### Release-notes wording (proposed)

> This release records the embedding-runtime identity (`backend` + `dtype`,
> with the linked ONNX Runtime version embedded in the backend label) of
> every index generation and refuses to serve vectors across runtimes. The
> first vector search (or index update) on a pre-existing **local-model**
> workspace fails once with `ZG.ENGINE.INDEX_BACKEND_MISMATCH` and a message
> naming the mismatch; run `zg --index --rebuild <root>` to recover. Full-text
> search is unaffected throughout. Remote-model (qwen) workspaces are
> unaffected.

### Rollback (verified against the pre-feature code)

A pre-feature binary (before #78) **can** read a manifest rebuilt by the new
runtime — its `EmbeddingModelInfo` has no `backend`/`dtype` fields and no
`deny_unknown_fields`, so the new keys are dropped on decode — and it serves
the vectors **without any identity check**. Rolling back therefore restores
service without an error, but it silently reintroduces exactly the
cross-runtime embedding drift this feature exists to refuse: vectors rebuilt
by the new runtime get queried with the old runtime's embeddings. Treat
rollback as emergency-only, acceptable for FTS traffic and as a brief
stopgap for vector traffic, not as a steady state.

One hard caveat: an old binary's **incremental refresh re-serializes the
manifest from its own identity-stripped struct**, erasing the recorded
`backend`/`dtype`. If any rebuilt workspace is incrementally refreshed during
the rollback window (including auto-update), the re-upgraded binary sees it
as identity-less again and refuses until it is rebuilt a second time. Disable
auto-update on rebuilt workspaces for the rollback window, or budget for a
second rebuild pass after re-upgrading.

Rolling back across #94 behaves differently, and more safely: a pre-#94
binary serving an index rebuilt by a post-#94 runtime fails the identity
check cleanly — its runtime reports `onnx-cpu` where the manifest records
`onnx-cpu/ort-…`, an `INDEX_BACKEND_MISMATCH` demanding a rebuild — rather
than the pre-#78 rollback's silent drift. Rolling back to a pre-#94 binary
therefore refuses vector search on rebuilt workspaces instead of serving
mixed-runtime embeddings.

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
- Every runtime switch must be followed by a re-index of active workspaces
  (see "Index compatibility" above). The backend-identity refusal (see
  "One-time fleet reindex after backend identity") now enforces this for ORT
  version switches: the recorded backend label names the linked ONNX Runtime
  version (#94), so a switch to a different ORT version refuses to serve the
  old index. A same-version rebuild that still drifts embeddings reports
  identical labels and passes the check — the measured cases above are
  exactly that shape — so the manual re-index rule remains in force for
  those switches.

## Open items (explicitly out of scope for this change)

- Release artifact `zg-<version>-macos-arm64.tar.gz` + `SHA256SUMS` on
  `zg-v*` tags, including the `libonnxruntime.dylib` / `load-dynamic`
  packaging decision and codesign/notarization or the documented
  `xattr -d com.apple.quarantine` alternative (#75 scope item 1).
- Engine-level workspace-roots allowlist: the container wrapper enforced
  `ZVEC_GREP_WORKSPACE_ROOTS`; the native server trusts loopback callers the
  same way the container's MCP endpoint did. Needs an engine-level guard
  before any non-loopback exposure, or an explicit decision to trust
  localhost only (#75 scope item 4).
- GPU indexing backend (#74), batching/thread fixes (#73), worktree index
  seeding (#72).
