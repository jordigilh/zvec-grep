# Codegraph publication and structural decisions (2026-09-30)

## Publication contract

The default graph output remains `.zvec-grep/codegraph-v2.json.zst`; the
backwards-compatible plain `.json` artifact is still readable and remains
available for explicit output paths. A default refresh also writes
`.zvec-grep/codegraph-publication-v1.json`.

The publication manifest records:

- the stored graph path and SHA-256 digest;
- graph schema/version, relation-extractor generation, and `manifest_key`; and
- exactly the semantic call-facts sidecars accepted into that graph, including
  each sidecar byte digest and context fingerprint.

The graph is atomically renamed into place first and the manifest is atomically
renamed last. The manifest is therefore a commit marker, not a second graph
store: a reader that validates it can reject an interrupted update or a graph
and sidecar mix from different generations. Validation also checks the source
files represented by the graph and current package-manifest inputs. A graph
that used syntax fallback publishes an empty sidecar list; no semantic facts
are enabled by the publication feature itself.

Existing `read_codegraph` callers do not require the manifest. New consumers
should use publication validation when they need a consistent graph-plus-facts
snapshot.

## Structural gap decisions

These decisions keep the current four-language graph conservative until there
is independent truth data for a proposed expansion.

1. **Module and symbol import edges coexist.** Local module resolution produces
   `imports_from`/`re_exports` edges to source files; when the syntax names local
   declarations, additional `imports`/`re_exports` edges point to those symbols.
   External imports remain name-only package targets. The module edge is retained
   so callers can choose file-level or symbol-level granularity.
2. **Containment is syntax-backed.** `contains` edges connect a file or lexical
   declaration to each declaration it owns. No synthetic declaration nodes or
   guessed cross-file containment relationships are added as a parity shortcut.
3. **References and tests remain bounded.** `references` and `tests` edges are
   emitted only from source-anchored syntax facts and the current narrowly
   defined test-owner rules. Broader framework annotations require dedicated
   fixtures and explicit qrels.
4. **Inheritance, implementation, and embedding require explicit syntax.**
   Compiler-inferred relationships, including implicit interface satisfaction,
   are not promoted into structural edges by default.
5. **`overrides` and `mixes_in` remain reserved.** No four-language evidence in
   the current fixtures justifies guessed edges for either relation.
6. **Communities are a separate evidence task.** Leiden output is already
   deterministic; Graphify community summaries will be compared in a dedicated
   corpus rather than changing graph semantics to match an unscored summary.
7. **Lifecycle work is test expansion, not a refresh rewrite.** Current-source
   refresh, source stamps, ignore handling, and full/incremental parity remain
   the implementation baseline. Scanner, watcher, cache, reconciliation, and
   current-source lifecycle contracts are now covered by focused tests; broader
   comparator fixtures can be added without changing refresh semantics.

The next evidence tranche is independent multi-file semantic truth for Rust,
TypeScript/TSX, and Python, followed by the bounded structural and lifecycle
fixture expansions above. Large-repository throughput remains isolated in
issue #25.
