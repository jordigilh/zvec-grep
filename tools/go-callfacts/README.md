# Go call-facts sidecar generator

This opt-in helper generates source- and analysis-context-attested Go call
facts for zvec-grep's callgraph. It does not change semantic-search ranking.
The Rust graph builder consumes the sidecar automatically when its complete Go
source list and module/workspace input hashes match the checked source snapshot.

## Frozen fixture and truth

The synthetic module is in
`rust/crates/zg-codegraph/tests/fixtures/go-blast-radius/`. Its manually authored
`truth.json` distinguishes statically bound calls, interface dispatch, and
function-value calls. The fixture includes a cross-package same-name decoy,
same-name methods on distinct receiver types, a three-hop caller chain, and
dynamic-call cases. A second fixture,
`rust/crates/zg-codegraph/tests/fixtures/go-context-matrix/`, freezes Linux vs.
Windows file selection, build tags, generic calls, and external calls across two
Go analysis contexts.

## Generate an opt-in sidecar

From the repository root, run:

```sh
go -C tools/go-callfacts run . --root /absolute/path/to/go/workspace --write-sidecar
```

The selected root must have an active `go.mod` under it; unsupported moduleless
workspaces fall back to syntax-derived graph edges.

The helper uses `go/packages` and `go/types`. It writes
`/absolute/path/to/go/workspace/.zvec-grep/go-callfacts-v2.json` atomically only
after package loading/type checking succeeds. Interface dispatch is recorded as
possible concrete targets, function-value calls are unresolved, and files
excluded by the active Go build configuration receive unresolved call facts.
Remove the sidecar to disable the overlay. If source digests no longer match,
the recorded context fingerprint is inconsistent, any `go.mod`, `go.sum`,
`go.work`, or `go.work.sum` input changes, or the sidecar is malformed or
unsupported, Rust uses syntax-derived calls instead. Results include the
context fingerprint that was applied.

The sidecar records the effective Go toolchain version, module mode, target and
feature settings, `GOFLAGS`, workspace/module selection, and hashes of `go.mod`,
`go.sum`, `go.work`, `go.work.sum`, and `vendor/modules.txt` files under the
workspace root. Vendored `.go` sources are also included in the source snapshot.
Facts are **scoped to that generation context**:
Rust does not invoke Go to compare against a different environment. Regenerate
the sidecar whenever you change Go version, target, build flags/tags, workspace,
or dependency configuration. The helper rejects a custom `GOPACKAGESDRIVER`,
GOFLAGS `-overlay`, `-modfile`, `-toolexec`, and `-pkgdir` options, active cgo-
dependent packages, an active `go.work` outside the root, and local workspace or
replacement modules outside the root rather than emitting facts it cannot bind
to in-root inputs. Downloaded dependency contents, toolchain binaries, and
runtime interface behavior are not hashed; results do not claim portability
across those inputs or across Go build configurations.

## Rust graph-query test

From `rust/`, run:

```sh
cargo test -p zg-codegraph --test go_blast_radius_spike -- --nocapture
```

The Go tests compare emitted call facts with independently authored frozen
truth. Rust tests then consume a sidecar through the same builder used by CLI
and MCP and compare blast-radius results against the fixture truth.
