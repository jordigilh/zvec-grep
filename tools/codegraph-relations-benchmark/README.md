# Codegraph relation comparison benchmark

This is a **local-only** comparison runner for the checked-in four-language
relation fixtures. It runs Graphify `0.9.71` and zvec against copied source
inputs, validates the source hashes from each fixture's `truth.json`, and
reports relation-level precision/recall.

Graphify is not a zvec runtime, build, or CI dependency. Do not add this runner
to the normal test suite. Run it manually after building `rust/target/debug/zg`:

```sh
python3 tools/codegraph-relations-benchmark/benchmark.py \
  --output /private/var/folders/.../zvec-codegraph-relations \
  --repetitions 3 \
  --force
```

The scored overlap is intentionally limited to file/declaration relations with
the same endpoint contract in both tools:

- `imports_from`
- `re_exports`
- `inherits`
- `implements`

Calls, references, tests, Go embedding, container edges, and Graphify's
symbol-level import granularity are emitted under `unsupported_or_unscored` in
`results.json`; they are not silently counted as false positives or false
negatives. The oracle is independently authored fixture truth and is never
copied into either tool's input directory.

Outputs are `results.json`, `results.csv`, and repeat working directories. The
timings are local process wall time and are not a cross-machine performance
claim.
