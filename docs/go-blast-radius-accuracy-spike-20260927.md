# Go blast-radius accuracy spike and opt-in integration (2026-09-27)

## Scope

This spike targets `zvec_grep_callgraph_blast_radius`, not semantic-search
ranking. A small Go module with independently authored source-anchored truth
lives at `rust/crates/zg-codegraph/tests/fixtures/go-blast-radius/`. It includes
a cross-package same-name decoy, same-name methods on different receiver types,
a three-hop call chain, an interface call, and a function-value call.

`tools/go-callfacts` uses `go/packages` and `go/types` to produce the opt-in
`.zvec-grep/go-callfacts-v2.json` sidecar. The Rust graph builder consumes that
sidecar only when its source-file list, recorded analysis-context fingerprint,
and in-root module/workspace/vendor-manifest input hashes match; it also
validates each call site's byte span and caller identity against the parsed
graph. Stale, malformed,
unsupported, or inconsistent facts fall back to parser-derived edges. Go
loading/type-checking errors prevent a new sidecar from being written. Facts
are explicitly scoped to the captured Go environment; Rust does not invoke Go
to prove equivalence with another active environment.

The supported context contract records the Go toolchain version and
`GO111MODULE`, `GOOS`, `GOARCH`, `CGO_ENABLED`, architecture feature settings,
`GOEXPERIMENT`, `GOFLAGS`, and `GOTOOLCHAIN`; active module/workspace selection; and hashes of
in-root module/workspace files and `vendor/modules.txt`. Local workspace modules
and replacements must remain under the root. Rust independently validates the
serialized contract, fingerprint, in-root input set and digests, source list,
and call-site correspondence, but deliberately does not run Go to attest the
consumer's process environment. Results therefore identify their generation
context rather than claiming cross-context equivalence. Downloaded dependency
contents, toolchain binaries, and runtime dispatch are outside this attestation
boundary. The producer rejects unpinned GOFLAGS file/tool overrides and any
active package importing `C`, avoiding sidecars whose type information depends
on unrecorded overlay files, alternate module manifests, custom tool drivers,
or cgo sysroots.

## Results

### Target binding

The oracle matched all **8/8 statically bound calls** to the expected source
symbol and call-site byte offset. It separately classified the interface call
as `interface-dispatch` and the function-value call as `function-value`, rather
than asserting either was a direct call to a same-named function. The fixture's
single interface implementation is listed as a *possible* target in the frozen
truth, not as a compiler-proven runtime dispatch.

The existing name-based codegraph produced 7 definite target edges against 8
static call facts: **5 exact edges, precision 0.714, recall 0.625**. The errors
were concrete and reproducible:

- `dep.Flush()` in `app/calls.go` was attached to the same-file `app.Flush()`.
- `Alpha.Run()` and `Beta.Run()` were left ambiguous despite statically known
  receiver types.
- `Runner.Execute()` was incorrectly treated as a definite call to the only
  same-named concrete method, `Worker.Execute()`.

### Blast-radius traversal and integration

The spike exposed a second accuracy loss after edge extraction: the query index
collapsed same-file methods with the same name into one vertex, even though the
artifact had distinct receiver-qualified IDs. The index now preserves symbol
identity and only uses a qualified display suffix when a path/name is
ambiguous. `receiver_type` also strips the Go grammar's receiver parentheses,
so method identities are `app.Alpha.Run`, not `app.(Alpha).Run`.

The blast-radius result separates `callers_by_depth` (edges the artifact marks
resolved) from `possible_callers_by_depth` (paths that use an ambiguous candidate
edge). “Resolved” is not inherently a type-certainty promise; it becomes
type-aware for Go call sites covered by a valid sidecar. The Go producer
classifies interface calls as possible concrete targets and function-value
calls as unresolved. Go files excluded by the active build configuration get
unresolved call facts rather than name-only definite edges.

An additional independently labeled context matrix covers Linux versus Windows
file selection, default versus explicit build tags, generic function calls,
external calls, and inactive-file fallback. The Go producer matches this truth
under both build profiles. A CLI end-to-end check then generated each profile's
sidecar, built/refreshed the graph, and verified platform/tagged/generic
blast-radius callers and distinct context fingerprints. A Rust integration test
also freezes those per-profile results against the graph consumer.

All six source-pinned fixture blast-radius queries pass with facts consumed by
the Rust graph builder. The MCP blast-radius handler and CLI graph-build/query
path also passed the fixture truth, including the three-hop chain and the
interface caller's possible depths 1 and 2. Removing the sidecar invalidates the
query cache and restores syntax-derived results.

## Confidence update

These are engineering judgments, not statistical confidence intervals:

- **0.90** that the Go-aware producer improves static call binding for supported,
  successfully type-checked packages. It matches the fixture, but the sample is
  intentionally small.
- **0.95** that receiver-preserving traversal and separate definite/possible
  results produce the intended blast-radius answer when supplied those facts.
- **0.70** that the opt-in feature will be both precise and complete across real
  Go repositories. The sidecar is now wired and end-to-end tested, but build
  environments, incomplete packages, generated code, function values, and
  interface dispatch need broader coverage.

The overlay is opt-in: generate it explicitly with the Go helper. Do not make
generation automatic or claim repository-wide completeness until a broader
adversarial fixture set and real-repository evaluation pass.

## Verification

- `go -C tools/go-callfacts test ./...` and `go -C tools/go-callfacts vet ./...` — passed, including source truth, the two-profile context matrix, context fingerprints, vendor manifest tracking, unsupported-input rejection, external replacement rejection, deterministic output, and atomic sidecar-write tests.
- `cd rust && cargo test -p zg-codegraph` — passed (22 unit, 5 blast-radius fixture, and 1 context-matrix test), including context freshness/fallback and generic call-site handling.
- `cd rust && CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" cargo test -p zg-transport-mcp` — passed (34 tests, including the MCP blast-radius handler).
- `cd rust && CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" cargo test -p zg -- --test-threads=1` — passed, including CLI fixture queries and daemon lifecycle tests.
- `cd rust && CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" cargo clippy -p zg-codegraph -p zg-transport-mcp -p zg --all-targets -- -D warnings` — passed.
- `go -C tools/go-callfacts vet ./...` — passed.
- Manually generated each context-matrix profile sidecar with `tools/go-callfacts`, built a graph with `zg --graph`, and verified platform-specific, build-tagged, and generic blast-radius callers and reported fingerprints with `zg --graph-query`.
