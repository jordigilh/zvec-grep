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
`.zvec-grep/codegraph-v1.json` artifact containing:

- source files and SHA-256 digests;
- stable file- and symbol-scoped IDs;
- declaration nodes and source ranges;
- `defines`, `imports`, and `calls` edges; and
- unresolved and ambiguous call metadata.

The graph is separate from the semantic search index and is refreshed against
the live root, including uncommitted changes.

### 2. Syntax-first graph construction

Tree-sitter supplies the common structural baseline. The graph remains useful
without a language compiler or language server. Syntax call resolution is
intentionally treated as name-based evidence, not type certainty.

The current graph contains language-specific declaration kinds such as
functions, methods, classes, interfaces, types, aliases, enums, constants, and
variables where the parser exposes them. The current edge vocabulary is still
limited to `defines`, `imports`, and `calls`.

### 3. Optional semantic overlays

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

### 4. Graph queries

`CallGraphIndex` currently projects resolved and possible call edges into
separate directed graphs and provides:

- reverse blast-radius traversal by depth;
- definite callers separate from possible callers;
- shortest directed call paths; and
- deterministic Leiden communities.

The current blast-radius index is function/method-centric even though the
artifact contains broader code declaration nodes.

### 5. Freshness and lifecycle

The graph refresh path reuses unchanged files, applies added/modified/deleted
source deltas, and re-resolves incoming call edges against the current
definition set. Semantic overlays validate the complete language source set,
source bytes, context files, coordinates, and producer fingerprints before
being applied.

## Parity matrix

| Capability | Current state | Parity work |
|---|---|---|
| Four-language source snapshot | Implemented | Add broader conformance fixtures |
| Cross-file `defines`/`imports`/`calls` | Implemented structurally; Go semantic path validated | Validate Rust, TypeScript, and Python with multi-file truth |
| Definite/possible/unresolved calls | Implemented | Preserve status on every serialized edge and query surface |
| Blast radius / affected callers | Implemented for functions and methods | Add relation filters and general node targets |
| Shortest path | Implemented for calls | Generalize to selected relation types |
| Communities | Implemented with Leiden | Compare code-only community semantics and summaries |
| Inheritance/implementation/override/mixin edges | Not implemented | Add syntax facts and bounded semantic adapters |
| References and test relationships | Not implemented | Add source-anchored relation types |
| Node/neighbor/explain inspection | Partial | Add generic code-node query APIs and provenance output |
| Incremental refresh | Implemented | Add explicit watch/ignore/cache parity tests |
| Snapshot publication | Graph and sidecars are separately persisted | Publish a consistent graph/facts generation |
| Four-language semantic accuracy | Go cross-file evidence; other holdouts are single-file | Add independent multi-file benchmarks |

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

1. independently truth-labeled multi-file fixtures for all four languages;
2. source-anchored implementations of the agreed relation vocabulary;
3. generic node, neighbor, path, affected, and explanation queries;
4. deterministic communities and incremental refresh behavior;
5. one source/context-attested published snapshot; and
6. a differential report against Graphify plus separate semantic certainty and
   freshness evidence.

The implementation should remain opt-in until these gates pass.
