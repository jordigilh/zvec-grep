# Multilanguage callgraph benchmark

This benchmark runs the opt-in Rust, TypeScript, and Python call-facts
producers against source-pinned synthetic fixtures and compares their zvec
graphs with Graphify `0.9.71` code-only extraction.

The oracle is the fixture's independently authored `truth.json`; it is not
copied into any tool input. The benchmark reports definite static edges
separately from possible targets and non-definite call sites. Graphify has no
uncertainty class, so its emitted call edges are counted as definite for the
static precision/recall comparison and as unsafe promotions when the oracle
marks a site possible or non-definite.

Run from the repository root after building `rust/target/release/zg` (the
runner falls back to a debug binary when no release binary exists):

```sh
python3 tools/multilanguage-callgraph-benchmark/benchmark.py \
  --output /private/var/folders/.../zvec-callgraph-benchmark \
  --repetitions 5 \
  --force
```

The output contains `results.json`, `results.csv`, and repeat working
directories. Timing is process wall time on the local host and includes the
language producer's startup/compiler or language-server work. It is not a
cross-machine performance claim. Graphify remains an external comparator and
is not a zvec runtime dependency.

Fixtures:

- `rust/crates/zg-codegraph/tests/fixtures/rust-graphify-holdout-20260928/`
- `rust/crates/zg-codegraph/tests/fixtures/typescript-graphify-holdout-20260928/`
- `rust/crates/zg-codegraph/tests/fixtures/python-graphify-holdout-20260928/`

The Rust holdout includes imported aliases and same-name helper/decoy modules;
the TypeScript holdout includes imported aliases, same-name modules, and a
`.tsx` caller; the Python holdout is an importable package with helper/decoy
modules and imported aliases. These cases are source-pinned in `truth.json`
and are intended to distinguish syntax-only name matching from the semantic
call-facts overlay. Go uses the separate
`tools/go-callgraph-benchmark/benchmark.py` because its producer has a v2
sidecar and an independently built helper.
