# Upstream Sync Log

Per-tier take/drop record for syncing upstream `zvec-ai/zvec-grep` into this
fork. Standing policy: sync with upstream EXCEPT on conflicts and except where
upstream features impact our goals (codegraph lineage, compiler-attested
callfacts, our fusion, the quality lanes). Every commit is cherry-picked
INDIVIDUALLY and gated on byte-identical quality-lane baselines.

Prior tiers:

- **Tier 1 / 1b / 2 / 3** — PRs #44, #45, #46, #47 (safe fixes, models
  subsystem, daemon lifecycle, perf). Deliberately not taken: upstream graph
  stack, code-aware reranking, docs restructure.

## 2026-11-19 — Tier 4: `sync/upstream-tier4`

Base: fork `main` @ `ec3ef16` (post PR #53, FP16/v3 index format).
Upstream analysis point: past `7504720`, commits evaluated individually.

### 975a477 — fix(build): download ONNX Runtime archives from npmmirror (#437) — **DROP**

- **Diff:** 5-line URL change in `rust/scripts/onnxruntime-distributions.json`
  (GitHub release URLs → `registry.npmmirror.com/-/binary/onnxruntime/...`).
- **Reachability evidence:** the npmmirror URL resolves:
  `curl -I https://registry.npmmirror.com/-/binary/onnxruntime/v1.21.0/onnxruntime-linux-x64-1.21.0.tgz`
  → HTTP 302 → `cdn.npmmirror.com/...` → HTTP 200. The mirror itself works.
- **Why dropped:** the target file and its entire consumer machinery do not
  exist in this fork. Upstream's ORT-distribution pinning was introduced by
  `4def566 fix(ort): pin official 1.21 runtimes` + `e6298f5` (never taken in
  any tier): the JSON is consumed by upstream's
  `rust/crates/zg-engine/build.rs` + `rust/scripts/prepare-onnxruntime.py`,
  neither of which exists here (`zg-engine` has no `build.rs`; the fork pins
  `ort =2.0.0-rc.13` and builds on helios08 with the verified manual
  ORT 1.28.0 archive + `ORT_LIB_LOCATION` + `ORT_PREFER_DYNAMIC_LINK=1`
  workaround). Cherry-picking only the URL edit is inapplicable (modify/delete
  conflict against an absent file); taking it usefully would require importing
  the whole 1.21-pinning machinery, which conflicts with our pinned
  `ort =2.0.0-rc.13` / ORT 1.28.0 line — the stated drop condition.
- **Future note (out of scope):** the npmmirror mirror is a viable fork-owned
  candidate for eliminating the manual helios08 ORT archive dance, if we ever
  pin distributions ourselves at 1.28.0. Requires its own PR.

### cc3ff0f — test(watcher): synchronize control symlink retargeting (#406) — **DROP**

- **Diff:** 2 hunks (+32/−25) inside the test
  `polling_tracks_control_symlink_targets_without_following_source_links`
  (staging-directory rewrites, atomic `rename`-based control-link
  replacement, `wait_for_change_matching` instead of racy
  `wait_for_change` + immediate asserts).
- **Why dropped:** the test it modifies does not exist in this fork. Both it
  and `polling_ignores_excluded_symlink_loops_and_keeps_selected_aliases_live`
  were introduced by upstream `4a7f0d1 refactor(engine): simplify portable
  workspaces (#301)` — a deliberately not-taken engine refactor. The
  cherry-pick conflicts structurally: resolving it would mean importing ~150
  lines of tests from a skipped commit without the engine behavior they
  exercise. Test-only change with no target in our line; nothing to take.
  The underlying race-hardening idea is recorded should we ever write our own
  control-symlink watcher test.

### 051a70b — feat(install): add standalone GitHub Release installers (#377) — **DROP**

- **Diff:** 42 files, +6536/−108: new `rust/standalone/` install scripts
  (install.sh/install.ps1), `zg/src/standalone/*` (~2000 lines), a
  `zg-launcher` binary, 1522-line `tests/standalone.rs`, standalone
  manifest/publish/release scripts, and substantial edits to
  `zg-cli/src/install.rs`, `install/upgrade.rs` (+220), `install/upgrade/launch.rs`
  (+140), `zg-daemon/src/controller.rs` (+231), `zg/src/main.rs`, `zg/build.rs`,
  `zg/Cargo.toml`, npm release registry, and three CI workflows.
- **Why dropped:**
  1. **Conflicts with our distribution model.** This fork is container-first
     (Quay multi-arch images via `rust/scripts/container.sh`) plus a reworked
     npm package; we do not distribute standalone GitHub Release installers,
     and this feature is not additive to the paths we ship.
  2. **Structural cherry-pick conflicts — 18 files.** Modify/delete conflicts
     in core files the fork restructured away (`rust-publish.yml`,
     `rust-package-build.yml`, `install/upgrade.rs`, `upgrade_installed.rs`,
     cuda scripts, `npm-release-registry.test.mjs` — all deleted in HEAD), plus
     content conflicts in fork-diverged engine-adjacent code
     (`zg-daemon/controller.rs`, `zg-cli/src/lib.rs`, `zg/src/main.rs`,
     `zg/build.rs`) where our strict-freshness daemon line lives.
  3. It builds on upstream's install/upgrade machinery that no longer exists
     in this fork, so it cannot be taken without re-importing that machinery.

### Gate results

No commits were taken, so the branch contains **zero engine changes** relative
to `main` (docs-only delta). The qeval retrieval-quality lane baselines
(tuning 0.5887735493745122 / 0.7333333333333333 / 0.6770833333333334 / 0.25;
holdout 0.6308114991329224 / 0.78125 / 0.7833333333333333 / 0.3) cannot move;
CI lanes confirm on the PR. No helios08 build verification required: the
manual ORT 1.28.0 workaround is unchanged because 975a477 was dropped, and
`main` @ `ec3ef16` was already fully verified green there (PR #53).

**Tier 4 outcome: 0 taken, 3 dropped (all with structural evidence). The
fork remains as synced as the recorded policy allows.**
