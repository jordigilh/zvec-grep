# Rust `zg` release binary: Sense-inspired lexical baseline installed

**Outcome:** The executable the user specified,
`rust/target/release/zg`, has been replaced with a verified Rust release
build containing the previously missing Sense-inspired code-name **FTS**
projection. The Go/Code IR retrieval-accuracy spike remains parked. This is
**feature inclusion, not a reproduction of the TypeScript Go qeval scores**.
Rust uses its own extractor/indexer, so do not substitute the TypeScript
baseline nDCG/MRR/recall/precision figures for measurements of this binary.

## What was checked and changed

- Prior ignored executable: version `0.0.1`, modified 2026-09-24, SHA-256
  `d06179c411671e8e7730965def469edfb2f82751cee15f210a31ed288ec5a2b9`.
  It predates the 2026-09-25 TypeScript Sense-enhancement commit `5f2c5e4`.
  Its Rust source indexes bare code name/scope/signature/doc in FTS, not the
  Sense-enhanced `qualified:` and `name_parts:` terms. The old executable
  contains no `name_parts:` marker. Simply recompiling unmodified Rust would
  **not** deliver the requested baseline.
- Rust indexer now indexes source-attested `symbol:`, `qualified:`,
  `name_parts:`, `scope:`, `signature:` and `doc:` lexical lines from existing
  code metadata, with NFKC compatibility normalization, camel-case/acronym,
  underscore, Unicode letter/number and combining-mark tests derived from
  the TypeScript `identifierParts` contract. Vector metadata,
  source fragment, model, fusion and CLI search mode remain unchanged.
  The Rust metadata does **not** have the TypeScript `modifiers` field and the
  runtime/parsers are different; this is a focused port of the missing
  **searchable name projection**, not complete cross-runtime parity.
- Build revision: `dfb0552` (Rust implementation; branch based on fork
  integration, not the parked PR stack). A release build used offline locked
  dependencies, its own external Cargo target directory and temporary SDK
  C++ header flags. The original binary was copied, with its SHA verified, to
  `/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/zg-original-release-20260926`
  before only the requested ignored executable was atomically replaced.
  New release binary SHA-256:
  **`3a34905d6ecd25ffae79300afca5514595bad8bea8db50b90ef12873992efdf9`**.
  Installed binary, external candidate and version `0.0.1` were verified.

## Checks

`cargo fmt --all --check`, `cargo check --offline -p zg --all-targets` and
strict `cargo clippy --offline -p zg-engine --all-targets -- -D warnings`
passed. Full `zg-engine` unit run: **439 passed, 10 ignored**. All `zg`
binary/integration tests run serially: **61 passed**. The first *parallel*
`cargo test -p zg` attempt failed six daemon-lifecycle tests with an invalid
instance-lock record; a targeted test and the complete serial suite then
passed using an approved `TMPDIR`. The parallel failure is not hidden or
claimed fixed by the FTS patch. The local CLT `c++` initially could not find
`<cstdlib>`; supplying the installed macOS 27 SDK's libc++ include directory
via **command-scoped** `CXXFLAGS` resolved the build (no global SDK changes).

A fresh external smoke workspace indexed one Go file using the final rebuilt
release binary and a copied local CPU Potion model cache: 1 file, 2 entities,
0 failed. Running the **installed path** with `--mode direct --fts 'record
discovery'` returned `WorkflowState.RecordDiscovery` at rank 1 with its
original source lines. The new binary contains `name_parts:`; the old one
does not. This smoke check confirms executable/index/query wiring, not a
quantitative retrieval improvement over the old binary.

The build log, test logs and smoke workspace are under the same approved
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`
root. To rebuild from the committed Rust source on this host, run from `rust/`
with a **fresh** external Cargo target directory:

```sh
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX27.0.sdk \
CXXFLAGS='-isystem /Library/Developer/CommandLineTools/SDKs/MacOSX27.0.sdk/usr/include/c++/v1' \
CARGO_TARGET_DIR=/path/to/fresh-external-target \
cargo build --offline --locked --release -p zg
```

No frozen Engram fixture, model cache, or user's existing workspace
index was modified. **Existing indexes contain previously stored lexical
text:** run `zg --index <your-workspace> --rebuild --mode direct` with the
installed executable (and your normal model settings) before expecting the
new FTS projection there. A running Server-mode daemon must use/restart the
new executable independently; this work verified Direct mode only. No
Rust-versus-TypeScript paired Go qeval was run; no ranking-rollout claim.
