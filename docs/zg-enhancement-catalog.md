# `zg` enhancement catalog and upstream handoff

**Status:** authoritative inventory for the replayed `zg` change set

**Snapshot date:** 2026-10-01

**Snapshot commit:** `0b226ef` (`fix: preserve BuildKit automatic platform arguments`)

**Review base:** `origin/main` at `c1d2297`

**Review branch:** `replay/codegraph-issue-26`

This document is the index and change ledger for the enhancements currently
carried by this branch. The implementation and tests remain authoritative for
behavior; this catalog records the intended contracts, evidence boundaries,
compatibility decisions, and review slices so that the work can be upstreamed
without losing a feature or silently turning an experiment into a product
claim.

The snapshot is 49 commits and 255 changed files relative to `origin/main`
(79,884 additions and 80 deletions). The branch does not modify `origin/main`.

## Executive summary

This change set adds a local-first, multi-language codegraph to the Rust `zg`
implementation and exposes it through the CLI and MCP surfaces. It combines:

- structural graph extraction for Go, Rust, TypeScript/TSX, and Python;
- optional, source- and context-attested semantic call-facts overlays;
- definite versus possible call resolution, with conservative fallback;
- relation-aware node, neighbor, path, affected, explanation, and community
  queries;
- compressed graph persistence with plain-JSON compatibility;
- atomic graph-plus-sidecar publication and source/context validation;
- full-build/incremental-refresh lifecycle parity and cache freshness checks;
- indexed relation and call resolution that preserves graph bytes while
  reducing repeated candidate scans;
- source-pinned structural and semantic evidence, lifecycle fixtures, and
  local Graphify comparisons; and
- a QEMU-free UBI 10 multi-architecture runtime image for native `zg` MCP.

Raw benchmark JSON/CSV output remains outside the repository. The repository
keeps the test fixtures, independently authored qrels, validators, benchmark
runners, and result summaries needed to reproduce and review the contracts.

## What is a product contract versus evidence

### Product/runtime contract

The following behavior is implemented and tested in the Rust workspace:

1. A graph is scoped to an absolute workspace root and refreshes from current
   source, including uncommitted changes.
2. Structural graph edges are conservative and source-backed.
3. Semantic sidecars are optional. They are accepted only when their schema,
   complete source set, source digests, context inputs, locators, and ranges
   validate against the current graph snapshot.
4. Definite and possible targets are separate. A stale, malformed, or
   incomplete sidecar is rejected as a whole for that language and syntax
   edges remain available.
5. The default graph artifact is compressed, while existing plain JSON remains
   readable and explicitly writable.
6. A publication manifest can be validated as the commit marker for a
   graph-plus-sidecar generation.
7. MCP graph tools remain root-scoped and expose applied language/context
   metadata.
8. Graphify and compiler/analyzer producers are not runtime, build, or CI
   dependencies of `zg`.

### Evidence-only conclusions

The checked-in evidence supports bounded claims only. It does not establish
whole-repository accuracy, language completeness, or universal performance
superiority over Graphify. Graphify remains a local comparator. The benchmark
and qrel summaries below are intentionally separated from the runtime contract.

## 1. Public surfaces

### MCP tools

The default Rust agent toolset now exposes indexed search plus root-scoped
callgraph and codegraph operations:

| Family | Tools |
| --- | --- |
| Search | `zvec_grep_search` |
| Callgraph | `zvec_grep_callgraph_blast_radius`, `zvec_grep_callgraph_shortest_path`, `zvec_grep_callgraph_cluster`, `zvec_grep_callgraph_communities` |
| Generic graph | `zvec_grep_codegraph_capabilities`, `zvec_grep_codegraph_node`, `zvec_grep_codegraph_neighbors`, `zvec_grep_codegraph_relation_path`, `zvec_grep_codegraph_affected`, `zvec_grep_codegraph_explain` |

The `full` toolset additionally exposes graph/index administration documented
in [`docs/03-mcp.md`](./03-mcp.md) and [`docs/06-server.md`](./06-server.md).
The default agent surface intentionally omits destructive administration.

Every graph input carries an absolute `root`. Graph queries refresh the
current source snapshot and scope sidecars and in-memory caches to the
canonical root, so separate worktrees do not share graph state.

### CLI

The Rust CLI supports:

```text
zg --graph [root]
zg --graph-query <artifact> node <query>
zg --graph-query <artifact> neighbors <query> [--relation <kind>]
zg --graph-query <artifact> relation-path <source> <target>
zg --graph-query <artifact> affected <query> [--depth <n>]
zg --graph-query <artifact> explain <query>
```

The complete examples and option behavior are in [`docs/02-cli.md`](./02-cli.md).

## 2. Structural graph model

The serialized graph contract is:

```text
schema: zvec-grep.codegraph
version: 2
relation schema: zvec-grep.codegraph.relations v2
relation extractor generation: 4
default artifact: .zvec-grep/codegraph-v2.json.zst
legacy artifact: .zvec-grep/codegraph-v2.json
```

The versioned relation vocabulary is:

| Relation | Current policy |
| --- | --- |
| `defines` | Syntax-backed declaration ownership |
| `contains` | File/lexical containment of declarations |
| `imports` | Local symbol import or imported declaration relationship |
| `calls` | Syntax or accepted semantic call relationship |
| `inherits` | Explicit inheritance syntax where supported |
| `implements` | Explicit implementation syntax; no inferred Go interface satisfaction |
| `embeds` | Go embedding relationships |
| `imports_from` | Local module/file import relationship |
| `re_exports` | Explicit local re-export relationship |
| `overrides` | Reserved; not guessed |
| `mixes_in` | Reserved; not guessed |
| `references` | Bounded source-anchored references |
| `tests` | Bounded source-anchored test ownership |
| `depends_on` | Explicit package-manifest dependency only |

Language capability is reported per project rather than hiding tools from MCP
discovery. The current support matrix is conservative:

- common declaration, containment, call, reference, test, and manifest-backed
  dependency behavior covers Go, Rust, TypeScript/TSX, and Python;
- `implements` and `re_exports` are structurally supported for Rust and
  TypeScript/TSX;
- `embeds` is supported for Go;
- `depends_on` is read from explicit `go.mod`, `Cargo.toml`, `pyproject.toml`,
  and `package.json` declarations; and
- `overrides` and `mixes_in` remain reserved until independent evidence
  justifies them.

Local module relations deliberately retain both file-level and symbol-level
edges where available. Compiler-inferred relationships, implicit interface
satisfaction, synthetic containment, and guessed framework behavior are not
promoted into structural edges.

Detailed publication and structural decisions are recorded in
[`docs/codegraph-publication-and-structural-decisions-20260930.md`](./codegraph-publication-and-structural-decisions-20260930.md).

## 3. Semantic call-facts overlays

The optional language-neutral envelope is documented in
[`docs/codegraph-callfacts-contract-20260928.md`](./codegraph-callfacts-contract-20260928.md).
Each producer supplies:

- a schema and version;
- a canonical analysis context and context digest;
- the complete source-file set and SHA-256 bytes digests; and
- call locations, caller locators, source target spelling, definite targets,
  possible targets, and a resolution class.

The consumer recognizes these certainty classes:

| Class | Graph behavior |
| --- | --- |
| `static` | One source-attested definite target |
| `possible` / `ambiguous` | Separate possible candidates; never definite |
| `function-value` | Non-definite indirect call |
| `external` | Non-definite target outside the attested source set |
| `unresolved` | Non-definite call with no safe target |

The branch includes producers and validators for:

- Go v2 source-pinned call facts, including interface dispatch and function
  values;
- Rust v1 facts using the pinned `rustc-dev`/compiler context;
- TypeScript/TSX v1 facts using TypeScript compiler context; and
- Python v1 facts using the bounded Python language-server path.

Possible Go interface dispatch and Rust trait dispatch remain possible. Dynamic
or unsupported TypeScript and Python cases remain non-definite. The overlay
does not change search ranking and does not invoke a producer from the Rust
runtime.

## 4. Persistence, publication, and lifecycle

### Artifact compatibility

- New default writes use zstd-compressed
  `.zvec-grep/codegraph-v2.json.zst` at compression level 3.
- `read_codegraph` accepts the compressed artifact and legacy/plain JSON.
- Explicit output paths ending in `.json` retain plain-JSON compatibility.
- Refresh prefers the compressed artifact, migrates the old default after a
  successful write, and avoids deleting a legacy artifact before the new one
  is valid.

### Publication boundary

Default refresh also writes
`.zvec-grep/codegraph-publication-v1.json`. The manifest records the graph
path/digest, graph schema and relation generation, and the exact accepted
semantic sidecars with their byte digests and context fingerprints. The graph
is atomically renamed first and the manifest last, making the manifest a
generation commit marker. Readers that require a consistent graph-plus-facts
snapshot validate it; legacy graph readers do not need it.

### Incremental lifecycle

The source-pinned lifecycle fixture covers:

- initial build and persisted refresh;
- modify, add, delete, and rename transitions;
- generated-directory and dependency-directory ignores;
- source and sidecar cache freshness;
- publication-marker reconciliation;
- source/graph tamper rejection; and
- no-op byte stability.

Every incremental transition is compared with a fresh build. See
[`docs/codegraph-lifecycle-evidence-20260930.md`](./codegraph-lifecycle-evidence-20260930.md).

### Parallelism and indexes

The implementation parallelizes full multi-language parsing, Go parsing,
source hashing, and changed-file parsing. Relation resolution uses indexed
candidate names and qualified names rather than scanning every node for every
edge. Follow-up indexes cover parsed files, Go package directories, Rust source
roots, imports, and borrowed call-definition candidates. The refactors preserve
artifact bytes, candidate ordering, ambiguity ordering, and semantic behavior.

## 5. Validation and captured results

### Structural qrels

The independent real-repository structural witnesses cover `praxis-filter`,
`koku-operator`, and `kubernaut-console`. All positive/negative relation and
community-pair checks passed. The qrels and validator are in
[`docs/codegraph-real-repository-qrels-20260930.md`](./codegraph-real-repository-qrels-20260930.md)
and `tools/codegraph-relations-benchmark/`.

### Semantic qrels

Bounded real-repository semantic witnesses cover Go, Rust, TypeScript/TSX, and
Python. Every represented resolution class achieved 1.00 one-vs-rest precision
and recall, with exact static-target accuracy for the selected witnesses. The
results are documented in
[`docs/codegraph-semantic-real-repository-qrels-20260930.md`](./codegraph-semantic-real-repository-qrels-20260930.md)
and validated by `tools/codegraph-semantic-benchmark/`.

### Four-language holdouts

The cross-file holdout run recorded the following semantic precision/recall:

| Language | Call sites | Syntax-only P/R | Semantic P/R | Semantic class accuracy |
| --- | ---: | ---: | ---: | ---: |
| Go | 2,412 | 0.99 / 0.99 | 1.00 / 1.00 | not scored in this table |
| Rust | 8 | 0.75 / 0.50 | 1.00 / 1.00 | 8/8 |
| TypeScript/TSX | 10 | 1.00 / 0.29 | 1.00 / 1.00 | 9/10 |
| Python | 10 | 0.67 / 0.33 | 1.00 / 1.00 | 10/10 |

These are source-pinned holdout results, not repository-wide claims. The full
methodology and historical comparison are in
[`docs/multilanguage-callfacts-benchmark-20260928.md`](./multilanguage-callfacts-benchmark-20260928.md).

### Large-repository performance

Performance is tracked separately from correctness in issue [#25](https://github.com/jordigilh/zvec-grep/issues/25)
and [`docs/codegraph-scale-performance-20260930.md`](./codegraph-scale-performance-20260930.md).
The important captured results are:

- on the controlled `helios08` Linux corpus, indexed relation resolution
  reduced the plain graph median from 55.253 s to 48.044 s (1.15x, 13.0%);
- on the current macOS arm64 corpus, the indexed resolver reduced plain graph
  build time from 4.221 s to 2.630 s (1.60x, 37.7%) while preserving the
  artifact SHA-256; and
- the earlier candidate-index refactor preserved graph bytes and reduced the
  macOS plain syntax median to 4.105 s, with default compressed syntax at
  4.308 s.

These figures are same-host comparisons with different graph shapes and are
not a cross-machine performance promise.

### Test and quality gates

The replay verification recorded:

- focused codegraph coverage including lifecycle, relation, and semantic
  holdouts;
- full workspace Rust tests, strict Clippy, formatting, and diff checks;
- JavaScript lint, formatting, typecheck, coverage, tests, and package checks;
- independent qrel validation; and
- MCP/CLI parity and server lifecycle checks.

Run the current workspace gates from `rust/` with the repository's normal
Rust/JavaScript commands. The exact replay gate output and compatibility
boundary are preserved in
[`docs/codegraph-upstream-readiness-20260930.md`](./codegraph-upstream-readiness-20260930.md).

## 6. Native container and publication

The container work is documented in [`rust/CONTAINER.md`](../rust/CONTAINER.md),
[`rust/Containerfile`](../rust/Containerfile), and
[`.github/workflows/publish-container.yml`](../.github/workflows/publish-container.yml).

The resulting runtime image:

- targets `linux/amd64` and `linux/arm64`;
- builds native target artifacts on the build platform using Zig and the UBI 10
  C/C++ runtime sysroot, without QEMU;
- contains `zg`, the matching `libzvec_c_api.so`, and Jieba dictionary assets;
- keeps the Rust toolchain and source tree out of the runtime image;
- defaults to `zg --server --stdio`;
- supports loopback-only HTTP at `127.0.0.1:7999/mcp` for a separate service;
- persists workspace indexes and `/var/lib/zg` state through mounts; and
- publishes through Quay as `quay.io/jordigilh/zvec-grep`.

The published verification tag is
`quay.io/jordigilh/zvec-grep:zg-v0.0.1-test.3`, with native amd64 and arm64
manifest entries. The publication workflow is independent of the public
Engram gateway. A gateway or host bridge must preserve the loopback-only
default rather than changing `zg` to an unauthenticated `0.0.0.0` listener.

## 7. Compatibility and deliberate non-goals

- Existing plain JSON graph artifacts remain readable.
- The publication manifest is additive and optional for legacy readers.
- Missing or invalid semantic facts fall back to syntax-derived edges.
- Graphify is a local comparator only, never a runtime or build dependency.
- The implementation does not claim runtime dispatch, macro/generated-source
  expansion, closure identity, async lowering, arbitrary function-pointer
  targets, unsupported generic resolution, or external dependency source
  identity.
- The graph does not infer implicit interface satisfaction or invent
  `overrides`/`mixes_in` edges.
- Small qrels and benchmark fixtures are not whole-repository accuracy claims.
- Raw benchmark output remains external; result summaries and reproduction
  procedures are the retained record.

## 8. Repository map

| Area | Authoritative implementation/evidence |
| --- | --- |
| Graph model and persistence | `rust/crates/zg-codegraph/src/lib.rs` |
| Generic graph queries | `rust/crates/zg-codegraph/src/graph_queries.rs` |
| CLI graph commands | `rust/crates/zg-cli/src/lib.rs`, `rust/crates/zg-cli/src/render.rs` |
| MCP graph tools | `rust/crates/zg-transport-mcp/src/lib.rs` |
| Runtime/server wiring | `rust/crates/zg/src/main.rs`, `rust/crates/zg-engine/` |
| Language producers | `tools/go-callfacts/`, `tools/rust-callfacts/`, `tools/typescript-callfacts/`, `tools/python-callfacts/` |
| Structural qrels | `tools/codegraph-relations-benchmark/` |
| Semantic qrels | `tools/codegraph-semantic-benchmark/` |
| Lifecycle tests | `rust/crates/zg-codegraph/tests/codegraph_lifecycle_e2e.rs` |
| Relation tests | `rust/crates/zg-codegraph/tests/codegraph_relations_e2e.rs` |
| Container build | `rust/Containerfile`, `rust/container-build.sh`, `rust/scripts/container.sh` |
| Multi-arch CI | `.github/workflows/publish-container.yml` |
| Container operations | `rust/CONTAINER.md` |

## 9. Review and upstream handoff

The branch is intentionally kept as a replay branch rather than merged into
`origin/main`. The commit history is chronological, but the following review
slices identify the logical boundaries for a dedicated PR or later PR split:

| Slice | Commit range | Contents |
| --- | --- | --- |
| Codegraph foundation | `a80b437` through `fdae924` | Multi-language graph, callgraph certainty, Go context validation, generic queries, manifest dependencies, topology, and affected traversal |
| Four-language semantic/structural evidence | `a6397fb` through `5a40bf2` | Relation parity, cross-file semantic callfacts, and holdout coverage |
| Persistence/publication/lifecycle | `5233c86` through `669f553` | Parallel extraction, compressed artifacts, publication manifests, real qrels, and lifecycle differential tests |
| Resolver performance and replay verification | `69e41e3` through `756344e` | Throughput profiles, candidate/import/call indexes, byte-identity checks, and upstream handoff evidence |
| Native container publication | `55ac271` through `0b226ef` | UBI 10 runtime image, QEMU-free multi-arch CI, Quay publication, and BuildKit platform scoping |

The complete commit ledger below is the lossless inventory for this snapshot.
Any PR split should preserve these commits or explicitly record why a commit is
excluded.

## 10. Complete commit ledger

The following is `git log --reverse origin/main..HEAD` at the snapshot commit:

```text
a80b437 feat(codegraph): add multi-language graph tools
89bb934 feat(codegraph): separate possible callers in blast radius
5ab0992 feat(codegraph): consume source-pinned Go call facts
9155171 feat(go-callfacts): attest analysis context
fbd9b3f feat(codegraph): validate Go analysis context
e4877ef docs(callgraph): document Go context contract
24230e3 test(codegraph): freeze Graphify callgraph comparison
f5f127d bench(codegraph): add large Go timing fixture
b411078 feat(codegraph): add four-language semantic callfacts
25b2c43 feat(codegraph): add generic relation queries
c590cc9 test(codegraph): cover attribute-marked Rust tests
fbb5bbc fix(codegraph): satisfy strict relation lint
f809195 fix(codegraph): require regenerated snapshots and expose capabilities
2ffc90d ci(rust): check codegraph crate independently
07294e2 test(codegraph): stabilize Go fixture line endings
3a5d53d feat(codegraph): add manifest-backed depends_on edges
be0f3fb fix(codegraph): reject malformed TOML manifests
0f90658 fix(codegraph): reject invalid TOML escapes
f05aa7a fix(codegraph): validate TOML unicode escapes
4a7bb75 fix(codegraph): fail closed on malformed manifest bytes
57acf8a fix(codegraph): reject invalid TOML Unicode scalars
5cee7a3 fix(codegraph): reject incomplete manifest dependency lists
fdae924 feat(codegraph): add topology relations and affected traversal
a6397fb feat(codegraph): add four-language relation parity coverage
5a40bf2 test(codegraph): add cross-file semantic callfacts holdouts
9fe0599 docs(codegraph): include Go semantic benchmark row
5233c86 perf(codegraph): parallelize extraction and compress artifacts
4d2cfbb feat(codegraph): publish structural relation snapshots
93cc9a4 test(codegraph): add source-pinned repository qrels
b33d95d docs(codegraph): record lifecycle and benchmark evidence
25f10ab test(codegraph): add real-repository semantic qrels
669f553 test(codegraph): add lifecycle differential fixture
69e41e3 docs(codegraph): record validation and upstream slices
0db9df8 perf(codegraph): record current throughput profile
f169e30 perf(codegraph): index relation candidates
b918e01 docs(codegraph): record indexed resolver results
6abcf6d docs(codegraph): update upstream handoff
3d683e8 docs(codegraph): record helios08 indexed profile
0147b06 docs(codegraph): update helios08 handoff
e147ed8 test(codegraph): align replay compatibility expectations
edc06f8 test(codegraph): format semantic qrels
31a522d docs(codegraph): record replay verification
c0129f3 perf(codegraph): index import and call resolution
1c4d749 docs(codegraph): record resolver follow-up
f5bf164 docs(codegraph): record helios08 resolver benchmark
756344e docs(codegraph): compare helios08 Graphify timing
55ac271 ci: publish multi-arch Rust zg container
ccfee99 ci: build UBI10 multi-arch Rust zg without QEMU
0b226ef fix: preserve BuildKit automatic platform arguments
```

## 11. Remaining review items

Before upstream merge, reviewers should decide:

1. Which logical slices should be separate PRs versus one stacked review.
2. Whether the benchmark runners and source-pinned qrels belong in the upstream
   repository or in an evidence-only companion branch.
3. Which container registry/tag policy is acceptable for upstream CI.
4. Whether the current public MCP names and graph schema are ready for a
   compatibility commitment.
5. Whether the performance issue remains a deferred investigation after the
   indexed follow-ups.

Those are review and release decisions, not undocumented implementation gaps.
