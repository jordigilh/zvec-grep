# Upstream contribution proposals (2026-09-28)

## Recommendation

Propose **two independent upstream issues and pull requests**:

1. **Port the Sense-inspired lexical code-name projection to Rust.** This is a
   small, low-risk indexing change with a narrow compatibility surface.
2. **Add source-attested Go call facts to code-only blast-radius queries.** This
   is a larger structural feature with a producer, sidecar schema, provenance
   validation, certainty classes, fallback behavior, and new CLI/MCP behavior.

They should not share a PR. The first changes indexed lexical text; the second
adds a separate structural artifact and an optional Go analysis toolchain. A
maintainer can accept either independently.

**Status:** Local design proposal only. No issue or pull request has been
opened against `origin` (`zvec-ai/zvec-grep`).

## Proposal A — Sense-inspired lexical code retrieval

### Candidate upstream issue

**Title:** Port Sense-inspired qualified-name and identifier-part indexing to
the Rust engine

### Problem

Code searches often use a qualified symbol name, a partial camel-case token, an
acronym, or an underscore-separated identifier rather than the exact source
spelling. The Rust index already exposes code metadata to lexical indexing but
did not include the qualified-name and decomposed identifier-part terms used by
the Sense-inspired path.

### Proposed scope

- Add source-attested `qualified:` and `name_parts:` lexical terms alongside
  the existing `symbol:`, `scope:`, `signature:`, and `doc:` terms.
- Keep the vector metadata, embedding model, default fusion, and result
  contract unchanged.
- Match the identifier-part normalization contract, including camel-case,
  acronym, underscore, Unicode-letter, combining-mark, and NFKC cases.
- Add unit and index smoke tests, plus explicit guidance that existing indexes
  need rebuilding after the lexical projection changes.

### Evidence already available

- Implementation commits: `87bf286` and `dfb0552`.
- Release verification and rebuild boundary: `540ac22` and `76941c9`.
- `cargo fmt`, focused Rust tests, strict Clippy, release build, binary
  fingerprint checks, and direct indexed smoke search passed.

The evidence demonstrates that the projection is wired and searchable. It does
**not** yet establish a statistically meaningful retrieval improvement. The
upstream PR should therefore describe this as a lexical coverage/parity
improvement, not as a guaranteed ranking win.

### Acceptance gates

1. Existing Rust indexing and search tests remain green.
2. Identifier normalization has source-level tests for ASCII and Unicode edge
   cases.
3. A same-engine paired retrieval evaluation reports per-query and aggregate
   changes against the current Rust control, with frozen inputs and qrels.
4. Any ranking regression is visible before enabling or changing default fusion.

### Non-goals

- No embedding-model change.
- No default fusion-weight change.
- No claim of TypeScript/Rust implementation identity beyond the tested lexical
  contract.
- No claim that a lexical projection alone improves answer correctness.

## Proposal B — Source-attested Go call facts for blast radius

### Candidate upstream issue

**Title:** Add opt-in source-attested Go call facts for precise codegraph blast
radius

### Problem

The syntax-only codegraph can construct useful caller paths, but name-based
binding is unsafe for Go when a workspace contains same-name methods, imported
functions, interfaces, generics, or function values. A graph query should not
present an interface candidate or a closure invocation as a definite static
call.

### Proposed architecture

Keep the structural graph useful without Go installed, and add an explicit
semantic overlay:

1. **Producer:** an opt-in Go helper uses `go/packages` and `go/types` to emit
   call-site facts.
2. **Attestation:** the sidecar records source file hashes, Go toolchain/build
   settings, active module/workspace inputs, and a context fingerprint.
3. **Consumer:** Rust validates the complete source set, context files,
   fingerprints, symbol identities, and call-site ranges before applying facts.
4. **Certainty:** definite static callers are separate from possible interface
   callers; function-value and unsupported calls remain unresolved.
5. **Fallback:** stale, malformed, unsupported, or inconsistent sidecars fall
   back to syntax-derived edges rather than silently claiming semantic
   equivalence.
6. **Surfaces:** expose the distinction consistently through the codegraph
   artifact, CLI, and MCP blast-radius result.

### Evidence already available

The local implementation is represented by commits `f1b04cd`, `0125ccc`,
`474f1b9`, `bb62f64`, `7aface0`, `ce0ad6b`, and `5a26804`.

The small hand-authored fixture matches all 8/8 labeled static calls. The new
scale fixture contains 2,376 static calls, 12 interface call sites, and 24
dynamic/function-value sites:

| Path | Static precision | Static recall | Interface candidates | Dynamic calls promoted |
|---|---:|---:|---:|---:|
| Graphify 0.9.71 | 0.985 | 0.995 | 12/144 | 12/24 |
| zvec syntax-only | 0.990 | 0.990 | 12/144 | 12/24 |
| zvec Go type-aware | **1.000** | **1.000** | **144/144** | **0/24** |

On the 8,688-line fixture, five fresh repetitions measured median wall time of
1.409s for Graphify code-only extraction, 0.288s for zvec syntax-only graph
construction, and 0.683s for zvec's complete type-aware path. The detailed
method, CSV runner, fixture, and limitations are in
`docs/go-blast-radius-benchmark-20260928.md`.

The `1.000/1.000` result is a **fixture-bound static-edge result**, not a
repository-wide accuracy guarantee. The benchmark oracle is generated from an
explicit call specification; the smaller fixture provides the stronger
hand-authored review evidence. Graphify is a comparison point, not a dependency
or a claim of general superiority.

### Proposed upstream PR sequence

If maintainers accept the feature direction, split review into these logical
units:

1. **Codegraph contract and query certainty:** stable source ranges, explicit
   definite/possible/unresolved semantics, and syntax fallback.
2. **Go producer and attestation:** sidecar schema, context validation,
   unsupported-input rejection, atomic writes, and producer tests.
3. **CLI/MCP integration:** blast-radius output and context fingerprint exposure
   with cache freshness coverage.
4. **Benchmark/evidence:** small hand-authored truth, context matrix, scale
   timing fixture, and reproducible CSV/JSON runner.

If upstream does not want a separate producer binary, the first PR can still
land the certainty-aware graph contract and keep the Go facts adapter as an
optional follow-up.

### Acceptance gates

1. No semantic-search ranking change and no default invocation of Go tooling.
2. Exact source/context mismatch tests prove safe fallback.
3. Definite, possible, and unresolved results are independently testable.
4. Independent truth includes same-name methods, imported same-name functions,
   interfaces, generics, function values, build tags, and platform selection.
5. At least one broader real-repository or independently labeled multi-module
   evaluation precedes any completeness claim.
6. Performance is reported separately for syntax graphing, Go fact production,
   and Rust graph consumption.

### Non-goals

- No runtime dispatch guarantee for interfaces.
- No inference that a valid sidecar is portable across Go contexts or toolchains.
- No automatic installation or invocation of Go from Rust.
- No search-ranking or embedding changes.
- No claim of universal “100% accuracy.” The defensible claim is “100% static
  edge precision/recall on the frozen supported fixture.”

## Shared upstream submission checklist

- Rebase or cherry-pick onto a current upstream `main` snapshot.
- Reduce each proposal to reviewable commits without local fork-only history.
- Open separate issue discussions before submitting the PRs.
- Include exact commands, source/model/tool versions, and negative results.
- Keep local Engram/Kubernaut integration and Graphify comparison context in
  linked evidence documents, not as runtime dependencies.
- Ask maintainers whether the Rust Sense projection belongs in the existing
  Rust branch or should wait for the upstream indexing architecture.
- Ask maintainers whether Go facts should be a built-in optional tool, an
  external producer contract, or an experimental branch feature.

## Suggested issue labels

- Proposal A: `enhancement`, `rust`, `search`, `code-indexing`.
- Proposal B: `enhancement`, `codegraph`, `go`, `mcp`, `performance`.
