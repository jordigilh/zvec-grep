# Rust `zg`: bring the Sense-inspired indexed text into the binary

**Scope:** The user's executable is `rust/target/release/zg`, not the Node
`dist/cli/index.js`. This branch starts from the merged fork integration tip
to avoid depending on the parked IR/retrieval PR stack. Do not change default
fusion, model selection, IR ranking, frozen evaluation sources or qrels.

## Diagnosis and bounded implementation

- The existing Rust executable is version 0.0.1, SHA-256
  `d06179c411671e8e7730965def469edfb2f82751cee15f210a31ed288ec5a2b9`,
  built on 2026-09-24; the TypeScript Sense-inspired commit `5f2c5e4` was made
  2026-09-25. The Rust workspace at the current integration tip already
  includes symbol name, scope, signature and documentation in FTS input but
  lacks the TypeScript baseline's **qualified name and decomposed identifier
  parts**. A plain release rebuild is insufficient.
- Add Rust tests **first** for code identifiers (camel case, acronym boundary,
  underscores, Unicode letters and scoped names), source-preserving FTS
  projection, and a markdown/non-code regression. Implement the TypeScript
  baseline's `symbol`, `qualified`, `name_parts`, `scope`, `signature` and
  `doc` lexical lines where Rust metadata supplies them. Keep vector metadata
  unchanged; the Node enhancement deliberately adds name parts only to FTS.
  Rust `CodeMetadata` currently does not carry TS `modifiers`, so do not
  fabricate them; record this and any other differences rather than claiming
  full cross-runtime parity or transplanting TypeScript qeval scores.
- Run focused Rust unit tests, `cargo fmt --all --check`, `cargo check -p zg`
  and a release build in **separate external target storage**. Do not replace
  the existing ignored release executable on a failed build. Verify the new
  binary's version/usage and checksum, compare its indexer unit tests against
  the TS source text contract, back up the original executable to the approved
  external temp directory, then atomically replace **only**
  `rust/target/release/zg`. No other workspace indexes or user files will be
  deleted or rebuilt. Record old/new digests and actual test/build outcomes.

This is feature inclusion in the Rust executable, **not proof** that Rust
reproduces the observed TypeScript Go nDCG/MRR/recall/precision scores. The
Rust extractor/model and scoring semantics may differ; independent same-engine
Rust qeval against the frozen Go qrels would be needed for a numeric claim.
Existing Rust indexes need an explicit rebuild to contain the new FTS text;
a running daemon may need a restart. The Go/Code-IR accuracy research remains
parked pending external evidence and independently judged unseen tasks.
