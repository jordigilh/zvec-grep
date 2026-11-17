# Four-language codegraph relation comparison (2026-09-29)

The checked-in relation fixtures under
`rust/crates/zg-codegraph/tests/fixtures/codegraph-relations-20260928/` now
serve three deterministic gates:

1. the `zg-codegraph` test validates source hashes, structural relations,
   uncertainty cases, persisted JSON, and incremental-refresh parity;
2. the `zg` integration test validates CLI graph-query JSON; and
3. the MCP transport test validates structured neighbors, relation-path, and
   affected-query JSON.

The optional local comparator is:

```sh
python3 tools/codegraph-relations-benchmark/benchmark.py \
  --output /private/var/folders/.../zvec-codegraph-relations \
  --repetitions 3 \
  --force
```

It runs Graphify `0.9.71` only on copied source inputs. `truth.json` is kept
outside both tool inputs, and Graphify is not a runtime, build, or CI
dependency.

## Scored boundary

The comparator scores only relations whose endpoint contract overlaps in the
two graphs:

- `imports_from`
- `re_exports`
- `inherits`
- `implements`

The oracle is source-pinned and independently labeled. Calls, references,
tests, Go embedding, container edges, and Graphify's symbol-level import
edges are reported as `unsupported_or_unscored`; they are not silently turned
into false positives or false negatives.

On one local macOS arm64 repeat using the checked-in fixtures, both tools
matched every scored overlap that Graphify emitted: Go `imports_from`; Rust
`inherits` and `implements`; TypeScript/TSX all four scored relations; and
Python `imports_from` and `inherits`. zvec also matched the Rust local
`imports_from` and `re_exports` qrels. Graphify's Rust import output was
module-item granular and did not provide the zvec file-level re-export edge,
so those two Rust relations remain explicitly outside the measured overlap.

The JSON report records these boundary decisions and all raw relation counts;
timings in the CSV/summary are process wall time on the local host, not a
cross-machine performance claim.
