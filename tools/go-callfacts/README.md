# Go call-facts sidecar generator

This opt-in helper generates source-attested Go call facts for zvec-grep's
blast-radius graph. It does not change semantic-search ranking. The Rust graph
builder consumes the sidecar automatically when it is present and its complete
Go source list and SHA-256 hashes match the checked source snapshot.

## Frozen fixture and truth

The synthetic module is in
`rust/crates/zg-codegraph/tests/fixtures/go-blast-radius/`. Its manually authored
`truth.json` distinguishes statically bound calls, interface dispatch, and
function-value calls. The fixture includes a cross-package same-name decoy,
same-name methods on distinct receiver types, a three-hop caller chain, and
dynamic-call cases.

## Generate an opt-in sidecar

From the repository root, run:

```sh
go -C tools/go-callfacts run . --root /absolute/path/to/go/workspace --write-sidecar
```

The helper uses `go/packages` and `go/types`. It writes
`/absolute/path/to/go/workspace/.zvec-grep/go-callfacts-v1.json` atomically only
after package loading/type checking succeeds. Interface dispatch is recorded as
possible concrete targets, function-value calls are unresolved, and files
excluded by the active Go build configuration receive unresolved call facts.
Remove the sidecar to disable the overlay. If source digests no longer match,
or the sidecar is malformed, unsupported, or internally inconsistent, Rust
uses syntax-derived calls instead.

The helper needs the Go version declared in `go.mod` and its module dependencies.
The sidecar records the source snapshot it analyzed; it does not attempt to
model every Go build configuration or runtime interface target.

## Rust graph-query test

From `rust/`, run:

```sh
cargo test -p zg-codegraph --test go_blast_radius_spike -- --nocapture
```

The Go tests compare emitted call facts with independently authored frozen
truth. Rust tests then consume a sidecar through the same builder used by CLI
and MCP and compare blast-radius results against the fixture truth.
