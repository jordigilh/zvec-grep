# Go blast-radius comparison: Graphify vs. zvec (2026-09-28)

## Result

On this fresh, source-pinned synthetic Go fixture, zvec's opt-in Go type-aware
call graph matched all 10 independently labeled static call edges. Graphify
performed better than zvec's syntax-only fallback on the static-edge metric,
but it bound one same-named receiver call to the wrong method and emitted two
other false definite edges.

| System | Exact static edges | False positives | False negatives | Precision | Recall |
|---|---:|---:|---:|---:|---:|
| Graphify 0.9.71, code-only | 9/10 | 3 | 1 | 0.75 | 0.90 |
| zvec, syntax-only | 6/10 | 4 | 4 | 0.60 | 0.60 |
| zvec, Go type-aware | 10/10 | 0 | 0 | 1.00 | 1.00 |

These are fixture results, not repository-wide accuracy estimates or a claim
that Graphify is generally less accurate. The fixture has only 10 static edges
and deliberately targets binding and certainty errors.

## Frozen fixture and scoring

The new fixture is
`rust/crates/zg-codegraph/tests/fixtures/go-graphify-holdout-20260928/`.
Its `truth.json` records source hashes, source-line symbol identities, 10
definite calls, one interface-dispatch call with a possible concrete candidate,
two unresolved function-value calls, and expected blast-radius depths. The
source snapshot is frozen by those hashes. The Go `go/types` producer emitted
13/13 call sites matching the independently written truth classifications.

The corpus covers two same-named receiver methods, three `Clear` functions in
different packages, an interface call, a shadowed local function value, a
generic call, and a three-hop call chain. Graphify was run on a source-only copy
containing `go.mod` and the three `.go` files; the oracle was not in its input.
No document extraction, clustering, LLM backend, or search ranking was involved.

For the static-edge score, a predicted edge is the exact `(caller symbol,
target symbol)` pair. Graphify's emitted `calls` edges are treated as definite
because it does not expose a separate possible-caller class. For zvec, only
resolved direct edges count as static; ambiguous candidates are not promoted to
definite edges. Interface candidates are assessed separately below. Precision
is `TP / emitted static edges`; recall is `TP / 10 expected static edges`.

### Edge errors

Graphify's three false positives were:

- `AlphaCaller -> Beta.Convert`, where the typed receiver requires
  `AlphaCaller -> Alpha.Convert` (the corresponding expected edge is the one
  false negative).
- `PublisherCaller -> Message.Publish` as an unqualified call edge. This is a
  possible interface-dispatch candidate, not a statically bound target.
- `ShadowedDispatchCaller -> Dispatch`, although the local variable shadows the
  package-level function and invokes a closure.

Graphify correctly resolved both imported package calls (`wire.Clear` and
`rival.Clear`). Its `affected` query returned the three-hop `Root` chain
correctly. For the two duplicate `Convert` nodes, the name query was not unique;
using Graphify's node IDs showed no caller for `Alpha.Convert` and both callers
for `Beta.Convert`.

The zvec syntax-only fallback correctly resolved six static edges. It left both
receiver calls ambiguous, attached `wire.Clear` and `rival.Clear` to local
`app.Clear`, treated the interface call as definite, and attached the shadowed
closure call to `app.Dispatch`. With the valid Go facts sidecar, zvec resolved
the receiver and package calls correctly, kept the interface candidate
possible, and left both function-value calls unresolved.

## Blast-radius query comparison

| Target (depth) | Truth | zvec syntax-only | zvec type-aware | Graphify `affected` |
|---|---|---|---|---|
| `Root` (3) | `LevelOne`, `LevelTwo`, `LevelThree` | Exact | Exact | Exact |
| `Alpha.Convert` (1) | definite: `AlphaCaller` | possible: both receiver callers | definite: `AlphaCaller` | no callers |
| `Beta.Convert` (1) | definite: `BetaCaller` | possible: both receiver callers | definite: `BetaCaller` | both receiver callers |
| `Message.Publish` (2) | possible: `PublisherCaller`, then `PublisherCallerOuter` | same callers, marked definite | same callers, marked possible | same callers, no uncertainty class |
| `app.Clear` (1) | `LocalClearCaller` | local plus both imported callers | exact | exact |
| `wire.Clear` / `rival.Clear` (1) | corresponding imported caller | no caller | exact | exact |
| `Dispatch` (1) | no caller | false `ShadowedDispatchCaller` | no caller | false `ShadowedDispatchCaller` |

The key distinction is certainty, not just caller recall: the type-aware result
keeps interface dispatch in `possible_callers_by_depth`, while the syntax-only
and Graphify results present that relationship as an ordinary call edge.

## Reproduction details

- Graphify package: `graphifyy==0.9.71`, installed/run with `uv tool run`.
- Graphify repository tag: `v0.9.71`, commit
  `d6eaa8aae8df155874ebb1044302c055c286342a`.
- Graphify mode: `extract <source-only-root> --code-only --no-cluster`.
- zvec source commit: `7aface04e6d36c0b1cda3fcb0c92f40f021fcf8c`.
- Go context: `go1.26.0`, `darwin/arm64`; sidecar context fingerprint
  `53b0b14e5324f5f56d940624715b200f9c5b97d9e311ae6ce7f28098de802d89`.
- zvec generated graphs contained 31 nodes and 41 edges for the 3 Go source
  files; the Graphify graph contained 32 nodes and 45 edges for its 4 source
  inputs (`go.mod` plus the 3 Go files). Graphify emitted 12 `calls` edges.

From the repository root, the core commands are:

```sh
FIXTURE=rust/crates/zg-codegraph/tests/fixtures/go-graphify-holdout-20260928
WORK=/path/to/temporary/go-callgraph-holdout

mkdir -p "$WORK/source-only"
cp "$FIXTURE/go.mod" "$WORK/source-only/"
cp -R "$FIXTURE/app" "$FIXTURE/wire" "$FIXTURE/rival" "$WORK/source-only/"

uv tool run --from 'graphifyy==0.9.71' graphify extract \
  "$WORK/source-only" --code-only --no-cluster --out "$WORK/graphify"

cp -R "$WORK/source-only" "$WORK/type-aware"
cp -R "$WORK/source-only" "$WORK/syntax-only"
go -C tools/go-callfacts run . --root "$WORK/type-aware" --write-sidecar
CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" \
  cargo run --release --manifest-path rust/Cargo.toml -p zg -- --graph "$WORK/type-aware"
rust/target/release/zg --graph "$WORK/syntax-only"
```

Then use `rust/target/release/zg --graph-query <artifact> blast-radius <symbol>
--depth <N>` and Graphify's `affected <node> --relation calls --depth <N>`.
All generated graphs and sidecars used for this run were kept outside the
repository.

## Limits and next gate

This is one small, deliberately adversarial fixture. It demonstrates that
Graphify's deterministic Go call extraction can outperform zvec's syntax-only
fallback on this sample, and that source-pinned Go type facts improve the
measured static bindings and preserve interface uncertainty here. It does not
establish general superiority, completeness, build-context behavior, or
real-repository accuracy for either tool. A broader independent corpus with
multiple modules and contexts is still required before making those claims.
