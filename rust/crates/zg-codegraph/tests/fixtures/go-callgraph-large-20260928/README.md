# Large Go callgraph fixture

This is a deterministic scale workload for the Graphify/zvec comparison. It
contains 12 feature packages, repeated cross-package calls into `common`,
same-name receiver methods, interface dispatch, generic calls, shadowed local
function values, and unresolved function-value calls.

`truth.json` is generated from the fixture generator's explicit call
specification and pins every source file with SHA-256. The benchmark runner
copies only `go.mod` and `.go` files into each tool's input directory, so the
oracle is never indexed.

This fixture is intended for throughput and high-volume edge-binding checks.
Use `go-graphify-holdout-20260928` for the smaller hand-authored accuracy
fixture and its detailed source-level review.
