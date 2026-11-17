# Four-language code-graph parity design (2026-09-28)

## Scope

This document defines the code-only parity target for the four languages under
active implementation:

- Go
- Rust
- TypeScript/TSX
- Python

Documents, media, and other non-code objects are deliberately out of scope;
Hindsight owns those concerns. Other programming languages are also out of
scope for this parity effort.

The target is parity with Graphify's **code graph** capabilities, not byte-for-
byte equality with Graphify's node IDs or an assertion that either graph is
complete for every language construct.

## Comparator boundary

The comparison baseline is Graphify `0.9.71` in deterministic
`--code-only --no-cluster` mode. The relevant code-graph capabilities are:

- source-file and declaration nodes;
- cross-file `calls`, `imports`, and code-structure relationships;
- path and affected/caller traversal;
- node, neighbor, and explanation-style inspection;
- communities and graph-level summaries; and
- refresh of a code snapshot after source changes.

Graphify remains an external comparator only. It is not a zvec runtime
dependency.

## Design implemented so far

### 1. Root-scoped structural snapshot

`zg-codegraph` recursively scans the selected checkout for Go, Rust,
TypeScript/TSX, and Python source. It persists a versioned
`.zvec-grep/codegraph-v2.json.zst` artifact. It uses the same JSON schema in
zstd-compressed form; legacy `.json` artifacts remain readable. It contains:

- source files and SHA-256 digests;
- stable file- and symbol-scoped IDs;
- declaration nodes and source ranges;
- `defines`, `contains`, `imports`, `imports_from`, `re_exports`, `calls`, `inherits`,
  `implements`, Go `embeds`, `references`, and `tests`, and explicit-manifest
  `depends_on` edges; and
- unresolved, ambiguous, and syntax-derived relation metadata.

The relation vocabulary is versioned independently as
`zvec-grep.codegraph.relations` v2 and currently admits `defines`, `contains`, `imports`,
`imports_from`, `re_exports`, `calls`, `inherits`, `implements`, `embeds`,
`overrides`, `mixes_in`, `references`, `tests`, and `depends_on`. Older local
v1 snapshots are not a compatibility contract; unknown future relation strings
are preserved by the edge envelope rather than discarded. The v2 artifact
requires a relation-extraction generation marker, so users must regenerate the
graph when moving to v2.

The graph is separate from the semantic search index and is refreshed against
the live root, including uncommitted changes.

### 2. Syntax-first graph construction

Tree-sitter supplies the common structural baseline. The graph remains useful
without a language compiler or language server. Syntax call resolution is
intentionally treated as name-based evidence, not type certainty.

The current graph contains language-specific declaration kinds such as
functions, methods, classes, interfaces, types, aliases, enums, constants, and
variables where the parser exposes them. The current structural extractor
emits the supported subset of the versioned relation vocabulary. Its
evidence-backed language capability matrix is:

| Relation | Structural producer languages | Current status |
| --- | --- | --- |
| `defines`, `contains`, `imports`, `calls`, `inherits`, `references`, `tests` | Go, Rust, TypeScript/TSX, Python | Supported; local symbol imports are syntax-backed |
| `implements` | Rust, TypeScript/TSX | Supported only for projects containing one of these languages |
| `imports_from` | Go, Rust, TypeScript/TSX, Python | Syntax-backed; local file targets resolve when the module path is supported |
| `re_exports` | Rust, TypeScript/TSX | Syntax-backed public/module re-exports |
| `embeds` | Go | Syntax-backed anonymous struct fields |
| `depends_on` | Go, Rust, TypeScript/TSX, Python | Supported from explicit package manifests |
| `overrides`, `mixes_in` | None | Reserved; no guessed edges |

Go intentionally reports `implements` as unsupported rather than treating
implicit interface satisfaction as a structural edge. `overrides` is reserved
for all four languages until a producer supplies source/compiler-attested
evidence. `depends_on` is emitted only from explicit `go.mod`, `Cargo.toml`,
`pyproject.toml`, or `package.json` declarations; it is never inferred from
imports, names, or directory layout. The root-scoped MCP capability result and
generic query metadata
report `supported`, `unsupported`, or `reserved` for each relation; they do not
hide the generic query tools based on project language.

Independent multi-file truth fixtures for the supported subset live under
`rust/crates/zg-codegraph/tests/fixtures/codegraph-relations-20260928/{go,rust,typescript,python}`.
Each fixture is source-hash pinned and keeps topology qrels, affected-query
qrels, absent relations, and ambiguous/unresolved call cases separate from the
Graphify comparator. The structural builder, persisted artifact, CLI JSON, MCP
structured JSON, and incremental refresh path are asserted against those
qrels. The optional local comparator is documented in
`docs/codegraph-relations-benchmark-20260929.md`; it scores only explicit
Graphify/zvec endpoint overlaps and reports the rest as unsupported or
unscored.

### 3. Explicit package-manifest dependencies

Package nodes use the manifest package name as their stable identity. Dependency
targets without a local manifest remain name-only package nodes, while edges
retain `resolution: "manifest"` and are refreshed when any recognized manifest
changes. Malformed or unsupported manifest content produces no dependency edge.
This is explicit configuration evidence: it proves that a package declares the
dependency, not that every source import resolves to it.

### 4. Optional semantic overlays

Each language has an independent, opt-in sidecar with exact source locators,
source hashes, analysis-context hashes, and explicit resolution classes:

| Language | Producer | Semantic source |
|---|---|---|
| Go | `go/packages` + `go/types` | active module/workspace context |
| Rust | pinned `rustc_driver`/HIR wrapper | Cargo and pinned rustc context |
| TypeScript/TSX | TypeScript compiler API | project/config/compiler context |
| Python | AST + pinned Pyright LSP | project/Python/typeshed context |

The common certainty model is:

- `static`: one source-attested target;
- `possible`/`ambiguous`: candidate targets without a definite binding;
- `function-value`: indirect callable or closure;
- `external`: target outside the attested source snapshot; and
- `unresolved`: insufficient safe evidence.

Valid facts replace only matching syntax call sites. Missing, stale, malformed,
or inconsistent facts fall back to syntax edges. This permits semantic
improvement without making compiler tooling a runtime requirement.

### 5. Graph queries

`CallGraphIndex` currently projects resolved and possible call edges into
separate directed graphs and provides:

- reverse blast-radius traversal by depth;
- definite callers separate from possible callers;
- shortest directed call paths; and
- deterministic Leiden communities;
- generic node inspection and explanation with manifest/context provenance;
- root-scoped language and relation capabilities;
- incoming/outgoing neighbor queries with relation filters; and
- relation-filtered shortest paths across every serialized edge kind; and
- generic reverse affected-node traversal with separate definite and possible
  results across selected relation kinds.

The current blast-radius index is function/method-centric even though the
artifact contains broader code declaration nodes.

### 6. Freshness and lifecycle

The graph refresh path reuses unchanged files, applies added/modified/deleted
source deltas, and re-resolves incoming call edges against the current
definition set. Semantic overlays validate the complete language source set,
source bytes, context files, coordinates, and producer fingerprints before
being applied.

## Parity matrix

| Capability | Current state | Parity work |
|---|---|---|
| Four-language source snapshot | Implemented | Add broader conformance fixtures |
| Cross-file `defines`/`imports`/`calls` | Implemented structurally; four-language source-pinned topology/query qrels and bounded real-root semantic qrels validated | Broaden independent semantic qrels and toolchain/context coverage |
| Definite/possible/unresolved calls | Implemented | Preserve status on every serialized edge and query surface |
| Blast radius / affected callers | Implemented for functions and methods | Preserve certainty separation and source provenance |
| Shortest path | Implemented for calls | Generalize to selected relation types |
| Communities | Implemented with Leiden | Compare code-only community semantics and summaries |
| Inheritance/implementation/embedding edges | Implemented for syntax-attested multi-file fixtures | Add bounded semantic adapters and broader language coverage |
| Package-manifest `depends_on` edges | Implemented for explicit Go, Rust, Python, and TypeScript/TSX manifests | Add broader manifest-format conformance fixtures |
| Override/mixin edges | Reserved; not inferred | Add only where syntax/compiler evidence is defensible |
| References and test relationships | Implemented for source-anchored syntax facts | Add broader annotation/test-framework coverage |
| Node/neighbor/path/explain/affected inspection | Implemented | Add richer edge provenance; publication validation is now available |
| Incremental refresh | Implemented; fixture-level full/incremental parity is checked for all four lanes, including changed target declarations and symbol imports; scanner, watcher, ignore, cache, reconciliation, current-source, and source-pinned lifecycle differential tests pass | Extend transition coverage only when a new lifecycle contract is introduced |
| Snapshot publication | v1 publication manifest records the graph digest and accepted sidecar generation | Adopt publication validation in consumers that require a committed graph/facts snapshot |
| Four-language semantic accuracy | Bounded real-root qrels report 1.00 class and exact static-target accuracy for all four opt-in producers; fallback remains conservative | Add broader repositories, toolchains, and language-specific uncertainty qrels before completeness claims |

## Opportunities to surpass Graphify

Parity does not require copying Graphify's uncertainty behavior. The zvec
design can provide stronger code answers by retaining:

1. **Resolution strength:** definite, possible, ambiguous, external, and
   unresolved edges instead of one undifferentiated call relation.
2. **Provenance:** producer, toolchain/configuration, source range, and context
   fingerprint for each semantic overlay.
3. **Freshness safety:** reject facts from a different source or build context
   rather than silently attaching them to the live checkout.
4. **Compiler-backed binding:** use Go, Rust, TypeScript, and Python semantic
   producers where available, with syntax fallback for unsupported constructs.
5. **Source-grounded integration:** connect graph results to zvec's indexed code
   retrieval without treating graph edges as search-ranking assumptions.

These advantages must be reported against independent truth fixtures and not
generalized from a small synthetic corpus.

## Non-goals

- Supporting languages outside Go, Rust, TypeScript/TSX, and Python in this
  parity effort.
- Implementing Graphify's document, PDF, image, video, or LLM extraction.
- Runtime dispatch guarantees, complete macro/generated-source expansion,
  closure identity, or external dependency source identity.
- Making Graphify a runtime dependency.
- Claiming repository-wide or whole-language accuracy from fixture results.

## Acceptance direction

The parity work is complete when the four-language code graph has:

1. independently truth-labeled, source-pinned multi-file fixtures for all four languages;
2. source-anchored implementations of the supported relation vocabulary, with
   reserved relations added only when evidence is defensible;
3. generic node, neighbor, path, affected, and explanation queries;
4. deterministic communities and incremental refresh behavior;
5. one source/context-attested published snapshot; and
6. a differential report against Graphify plus separate semantic certainty and
   freshness evidence; and
7. deterministic persisted, CLI, MCP, and incremental-refresh qrel coverage
   that does not require Graphify in CI.

The implementation should remain opt-in until these gates pass.
