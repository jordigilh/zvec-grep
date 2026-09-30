# Codegraph upstream-readiness review (2026-09-30)

This review is read-only: it does not rebase the current branch, modify
`origin/main`, or push any remote. The current branch is
`spike/codegraph-graphify-followup`, sixteen commits ahead of
`fork/integration/zvec-live-code-intelligence`:

```text
07f67d6 feat(codegraph): add topology relations and affected traversal
6577a07 feat(codegraph): add four-language relation parity coverage
6c5664a test(codegraph): add cross-file semantic callfacts holdouts
1153a05 docs(codegraph): include Go semantic benchmark row
ab03842 perf(codegraph): parallelize extraction and compress artifacts
7ef30d2 feat(codegraph): publish structural relation snapshots
ac50fc7 test(codegraph): add source-pinned repository qrels
3f38d57 docs(codegraph): record lifecycle and benchmark evidence
2bf2ddd test(codegraph): add real-repository semantic qrels
12affa7 test(codegraph): add lifecycle differential fixture
862fde9 docs(codegraph): record validation and upstream slices
61428c1 perf(codegraph): record current throughput profile
a29787d perf(codegraph): index relation candidates
38a6d18 docs(codegraph): record indexed resolver results
ac15f4f docs(codegraph): update upstream handoff
0d4d513 docs(codegraph): record helios08 indexed profile
```

The branch is also 72 commits ahead of the checked-out `origin/main`, because
the integration branch contains other fork work. A direct rebase would mix
unrelated history into this review and is intentionally deferred.

## Proposed logical replay slices

Replay or cherry-pick these slices onto a fresh branch based on the maintainer's
current target branch. The hashes below describe the current local history;
they are not an instruction to push them unchanged.

| Slice | Current commits | Review boundary |
| --- | --- | --- |
| relation/query contract | `07f67d6` | versioned relation kinds, generic graph queries, CLI/MCP wiring, affected traversal |
| structural truth and comparator | `6577a07` | four-language source-pinned relation/topology qrels and local-only Graphify comparison |
| semantic holdouts | `6c5664a`, `1153a05` | cross-file Rust/TypeScript/Python truth plus the existing Go evidence row |
| persistence/parallel extraction | `ab03842` | compressed default artifact, plain JSON compatibility, Rayon parsing/hash paths, benchmark updates |
| publication boundary | `7ef30d2` | accepted-sidecar manifest, atomic graph/manifest publication, source/context validation, lifecycle fallback |
| indexed relation resolution | `61428c1`, `a29787d`, `38a6d18`, `0d4d513` | throughput baseline, TDD-covered candidate indexes, byte-identical graph output, and same-host follow-up evidence |
| real-repository structural evidence | `ac50fc7` | revision- and source-digest-pinned witness qrels; no runtime dependency |
| evidence and handoff docs | `3f38d57`, `862fde9`, `ac15f4f` | structural decisions, same-host evidence, issue #25 baseline, and local integration status |

The follow-up is split into review units rather than folded into unrelated
product changes:

1. semantic qrel validator, four real-root qrels, and semantic evidence docs;
2. source-pinned lifecycle transition fixture and differential test;
3. indexed relation resolver and its throughput evidence;
4. this upstream-readiness review.

## Compatibility and dependency gates

- The default graph remains `.zvec-grep/codegraph-v2.json.zst`; explicit plain
  `.json` artifacts remain readable.
- The publication manifest is additive and optional for legacy readers. It is
  validated before consumers require a committed graph-plus-sidecar snapshot.
- Missing, stale, malformed, or parser-incompatible semantic sidecars fall
  back to syntax edges; no semantic producer is invoked by the Rust runtime.
- Graphify and the Go/Rust/TypeScript/Pyright producer toolchains remain in
  local-only evidence tooling, not in runtime, build, or CI dependency graphs.
- The new evidence tooling is Python-only and has no Cargo or application
  dependency changes.
- The lifecycle fixture proves add/modify/delete/rename/ignore/cache/
  reconciliation/full-vs-incremental/publication behavior independently of
  external analyzers.

## Verification at this boundary

The focused codegraph suite passed with 55 tests after the indexed resolver
test was added, including the lifecycle differential test. The four
real-repository
semantic qrel runs passed with class accuracy and exact static-target accuracy
of 1.00 on each bounded witness set. Full workspace verification passed with
`CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" cargo test
--workspace` (439 passed, 10 ignored), and strict workspace Clippy also passed.
No upstream PR is opened by this branch.
