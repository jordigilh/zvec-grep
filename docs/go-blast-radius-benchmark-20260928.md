# Go callgraph time and effectiveness benchmark (2026-09-28)

## Summary

The larger fixture confirms the expected throughput gap on this host. With
five fresh repetitions, Graphify's deterministic code-only extraction took a
median **1,409 ms**. zvec's syntax-only graph build took **288 ms**; zvec's
Go type-aware path took **683 ms end-to-end**, split into **266 ms** for the
prebuilt Go facts producer and **356 ms** for Rust graph construction.

| Path | Median | Min | Max | Relative to Graphify |
|---|---:|---:|---:|---:|
| Graphify 0.9.71 code-only extraction | 1,409 ms | 1,386 ms | 1,473 ms | 1.00x |
| zvec syntax-only graph | 288 ms | 284 ms | 366 ms | 0.20x |
| Go call-facts sidecar producer | 266 ms | 213 ms | 337 ms | 0.19x |
| zvec Go type-aware Rust graph | 356 ms | 354 ms | 417 ms | 0.25x |
| zvec Go type-aware end-to-end | 683 ms | 567 ms | 753 ms | 0.48x |

Thus Graphify was about **4.9x** slower than the syntax-only zvec graph and
about **2.1x** slower than zvec's complete type-aware path in this run. These
are process wall times on one macOS arm64 host, not portable performance
claims.

The one-time Go helper build took 745 ms in this run and is intentionally not
included in each repetition. Charging `go run` compilation to every query
would obscure the graph comparison; the benchmark reports that setup cost
separately.

## Workload

`rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928/` is an
8,688-line source snapshot across 94 module/source files (93 Go files plus
`go.mod`), containing:

- 12 feature packages and repeated cross-package calls;
- 2,728 functions/methods and other graph symbols;
- 2,376 expected static calls;
- 12 interface-dispatch call sites with 144 possible target relationships;
- 24 function-value/shadowed-call sites;
- same-name receiver methods, generics, imported calls, and repeated targets.

The oracle is generated from an explicit call specification, not from either
tool's output. It pins every input file with SHA-256. The benchmark copies
only `go.mod` and `.go` files into each tool's input root, so `truth.json` is
never indexed. This is a high-volume scale oracle; the smaller hand-authored
fixture remains the stronger independent review fixture for individual facts.

## Effectiveness

The exact static-edge results were:

| Path | True positives | False positives | False negatives | Precision | Recall |
|---|---:|---:|---:|---:|---:|
| Graphify 0.9.71 | 2,364 | 36 | 12 | 0.985 | 0.995 |
| zvec syntax-only | 2,352 | 24 | 24 | 0.990 | 0.990 |
| zvec Go type-aware | 2,376 | 0 | 0 | **1.000** | **1.000** |

Certainty handling was materially different:

| Path | Interface candidate relationships recovered | Dynamic calls promoted to definite |
|---|---:|---:|
| Graphify | 12/144 (0.083) | 12/24 |
| zvec syntax-only | 12/144 (0.083) | 12/24 |
| zvec Go type-aware | **144/144 (1.000)** | **0/24** |

Graphify's 36 false positives include one wrong `Alpha.Convert` binding, one
candidate per interface call, and one shadowed-call edge per feature package;
its 12 false negatives are the same-name `Alpha.Convert` calls that were
attached to `Beta.Convert`. The
syntax-only path has the expected receiver ambiguity and cross-package
same-name collisions. The type-aware path resolves all static calls, records
all interface implementations as possible, and abstains on function values.

## Earlier small-fixture timing

For context, the earlier three-file Go fixture was also measured once using
the already-built zvec binary and cached Graphify environment:

| Operation | Wall time |
|---|---:|
| Graphify code-only extraction | 0.71 s |
| zvec syntax-only graph | 0.14 s |
| zvec type-aware Rust graph | 0.13 s |
| Go facts plus zvec graph | 11.28 s |

The small-fixture Go number used `go run`, so **11.15 s** of it was first-run
Go helper compilation/startup. It should not be compared with the large
fixture's prebuilt-helper producer measurement. The large benchmark is the
preferred timing result.

## Reproduction and plotting

The generator and runner are in
`tools/go-callgraph-benchmark/`. The runner writes `results.csv` and
`results.json`; generated result directories stay outside the repository.

```sh
CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" \
  cargo build --release --manifest-path rust/Cargo.toml -p zg

python3 tools/go-callgraph-benchmark/benchmark.py \
  rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928 \
  --output /private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/zvec-go-callgraph-benchmark \
  --repetitions 5 --force
```

Plot `results.csv` using `wall_ms` grouped by `tool` and `phase`; plot
`precision`, `recall`, `possible_recall`, and `dynamic_promoted` as the
effectiveness series. The runner uses the cached `uv tool run
graphifyy==0.9.71` invocation and the already-built zvec binary. Report the
median/min/max, host, Go version, Graphify version, and cache state with any
plot.

## Limits

This is one generated scale workload, not a general benchmark suite. It does
not measure incremental updates, Graphify's LLM/document modes, disk-cache
reuse, Windows/Linux build contexts, or real repository distributions. The
next useful gate is several independently authored large modules with varied
package topology and build configurations.
