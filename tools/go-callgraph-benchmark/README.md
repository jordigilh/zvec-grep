# Go callgraph benchmark

This directory contains a deterministic scale fixture generator and a runner
for comparing Graphify with zvec's syntax-only and opt-in Go type-aware graph
paths. The runner keeps `truth.json` out of every tool input copy.

## Generate the scale fixture

The checked-in fixture is about 8.7k Go lines across 93 Go files, with 2,728
symbols and 2,412 labeled call sites:

```sh
python3 tools/go-callgraph-benchmark/generate_large_fixture.py \
  rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928 \
  --force
```

The generator uses an explicit call specification rather than reading any
Graphify or zvec output. `truth.json` freezes source SHA-256s, static calls,
interface candidates, and dynamic/function-value sites. It is a scale oracle;
the smaller hand-authored fixture remains the stronger independently reviewed
accuracy fixture.

## Run the benchmark

Build zvec first, then run several fresh repetitions. The Go producer is built
once outside the repetitions, so the measured type-aware end-to-end number
does not charge Go compiler startup on every sample:

```sh
CXXFLAGS="-isystem $(xcrun --show-sdk-path)/usr/include/c++/v1" \
  cargo build --release --manifest-path rust/Cargo.toml -p zg

python3 tools/go-callgraph-benchmark/benchmark.py \
  rust/crates/zg-codegraph/tests/fixtures/go-callgraph-large-20260928 \
  --output /private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/zvec-go-callgraph-benchmark \
  --repetitions 5 --force
```

The runner records:

- `graphify/code-only-extract`: `uv tool run graphifyy==0.9.71 graphify extract`;
- `zvec/syntax-only-graph`: Rust graph construction without a sidecar;
- `zvec-go-callfacts/produce-sidecar`: prebuilt `go-callfacts` generation;
- `zvec/go-type-aware-graph`: Rust graph construction with valid Go facts;
- `zvec/go-type-aware-end-to-end`: sidecar production plus graph construction.

`results.csv` is intended for plotting. It contains wall time in milliseconds,
source size, node/edge counts, exact static-edge TP/FP/FN, precision/recall,
interface-candidate recall, and dynamic-call promotions. `results.json` also
contains run metadata and the one-time producer build time.

The timing is process wall time on the local host. Graphify includes the
cached `uv tool run` launcher; zvec uses the already-built binary. Report
median, minimum, and maximum across repetitions rather than treating one run
as a benchmark constant. Go package/module caches and Graphify's installed
Python environment should be disclosed with the results.
