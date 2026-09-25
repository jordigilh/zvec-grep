# Patched scip-go revalidated against the committed Code IR design branch

**Contribution update:** The previously held upstream contribution is now [scip-go issue #297](https://github.com/scip-code/scip-go/issues/297) and [PR #298](https://github.com/scip-code/scip-go/pull/298). The revalidation below is the evidence linked from that human-review PR; it does not indicate an upstream merge or production rollout.

**Result (2026-09-25):** The condition for reconsidering a Go upstream contribution has been met **on the frozen development fixture**: the Code IR team committed and pushed its syntax-boundary/field changes to [`fork/spike/code-ir-design`](https://github.com/jordigilh/zvec-grep/tree/spike/code-ir-design) at `139fe4e7eda0c084e85eb641003a2e9204a87c60` (`cc26b79` source-evidence changes + `139fe4e` read-only recheck). Its compiled TypeScript frontend was **`web-tree-sitter-ir-v1.5`**, implementation SHA-256 `1f207a060556996c6b8043247b3235c8690763cc7169aef8e4d4b2f968b23130`. We independently reran the team's own **read-only** `scripts/code-ir-shadow-recheck.mjs` against the stock and patched Go producer artifacts on separate frozen copies with identical source bytes. No active worktree file was edited; the upstream scip-go proof remains uncommitted/unpushed and no PR was created.

| Same-source, same committed IR revision | Published `scip-go@v0.2.7` | Isolated patched upstream-main scip-go |
| --- | ---: | ---: |
| Go selected files, independent source-set digest | 10/10; `13ca96abdeb8ef919a746f1ec1afb3ae5b6d59e8b0282d98109bc2b6a8c4f53d` | Identical |
| Per-document encoding | 0/10 specified (pinned legacy compatibility rule) | **10/10 explicit UTF-8** |
| SCIP occurrences / definitions / references | 471 / 150 / 321 | **Identical** primary source/symbol/role tuples |
| Definitions with valid enclosing declaration spans | 27/150 | **58/150** |
| Normalized selected-local reference candidates | 18 | **117** |
| Current v1.5 syntax IR units / original syntax facts | **68 / 101** | **68 / 101** |
| Strict unique source/body/kind/reference joins, real runtime validated | **18/18 candidates** | **117/117 candidates** |
| Independently authored Go links retained | **5/5** | **5/5** |
| SCIP binary SHA-256 | `83313d8fc85730dee466c77d6f90a2445d054264f4f253f3327d1570120b22d3` | `b2049448085605274534ad1ed8851081f13e154dc04924a3fca6006d7bb4ee79` |

The stock source copy lives under `/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-stock-v15-20260925/`; the patched artifact is under `/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-enclosing-eval-20260925/go/index.scip`. The code-independent paired comparison reports **31 new valid enclosures**, no changes to existing enclosures and **zero changes to any of the 471 primary occurrence file/byte/symbol/role tuples**. The patched binary was built from isolated upstream-main checkout `343fa9ce00444421071d3659443a78cbfab3291f` plus the uncommitted visitor change; binary SHA-256 `eb09bc8ca1315c474308b9f6ca356f4ec744b45f9986519d65319c825b214e0e`. Despite that modified binary, its embedded SCIP ToolInfo still says `0.2.7`; attestation must key the actual executable hash/commit, not just ToolInfo.version. See [upstream experiment](scip-go-upstream-enclosing-spike.md) for changed files, snapshot updates, real Unicode/CRLF producer control and `go test ./...` results.

**What changed in Code IR:** v1.5 now preserves the `type` keyword/attached docs on canonical Go type units and emits source-backed named/embedded/multi-name struct-field `value` units. The team's shadow-recheck uses exact original source bytes, complete file/config/provenance checks and the independent v1.5 runtime validator. This resolves the previous v1.3 situation (patched producer's 117 local candidates but only 18 strict IR joins). It **does not** claim 100% of scip-go's 321 raw references: 204 do not have a unique selected-local declaration suitable for this import, including out-of-scope/external/local cases. Nor does it claim independently adjudicated precision of all 117 sites, implementation coverage, dependency-complete atomic snapshot attestation, ranking benefit or a production-default change. The five authored Go links are a small positive control, not a whole-corpus truth inventory.

For context, the pushed team's same-source Go LSP recheck has 62/117 direct canonical-body joins and **117/117 with a separately source-validated `syntax_source` alias**; that is an alternate-span diagnostic, not an automatic preference for SCIP based on raw count. Both systems can inform a shadow semantic importer. Do not run both by default solely to increase a number.

**Contribution decision:** a focused upstream follow-up to scip-go [#92](https://github.com/scip-code/scip-go/issues/92) is now supported by a passing upstream test suite **and** successful post-Code-IR fixture revalidation. A reviewer can separate the type/value/field enclosing-range extension from the one-line explicit UTF-8 position-encoding improvement; keep the generated expected snapshot annotations with the range change. Before publication, independently review a broader Go corpus and negative/malformed/generated-source cases and ensure the maintained branch still matches the pinned binary. At the user's direction, the upstream PR remains **on hold**; this spike does not commit, push or open it.

## Read-only reproduction against the pushed branch

Both commands below use the design checkout at committed `139fe4e` read-only; `--root` contains only independently staged fixture copies and pinned producer artifacts. A later compiled frontend digest should be treated as a **new** measurement.

```sh
# From /Users/jgil/go/src/github.com/jordigilh/zvec-grep-code-ir-design:
node scripts/code-ir-shadow-recheck.mjs --root /var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-stock-v15-20260925 --mode scip-go --language go
node scripts/code-ir-shadow-recheck.mjs --root /var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-enclosing-eval-20260925 --mode scip-go --language go --expect-scip-sha256 b2049448085605274534ad1ed8851081f13e154dc04924a3fca6006d7bb4ee79
```

`docs/scip-go-upstream-enclosing-spike.md` records the complete isolated upstream patch/test procedure. No Engram fixture files, Code IR team files or user defaults were changed for this verification.
