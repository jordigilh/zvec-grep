# Codegraph lifecycle differential evidence (2026-09-30)

The checked-in fixture at
`rust/crates/zg-codegraph/tests/fixtures/codegraph-lifecycle-20260930/` is a
source-pinned transition corpus. The integration test
`codegraph_lifecycle_e2e.rs` copies it into a temporary workspace and compares
every incremental transition with a fresh `build_codegraph` result.

## Transition matrix

| Transition | Incremental input | Differential assertion |
| --- | --- | --- |
| initial | full build and persisted refresh | compressed artifact and in-memory graph agree |
| modify | `Upsert(caller.go)` | modified incremental graph equals fresh build |
| add | `Upsert(added.go)` | added incremental graph equals fresh build |
| delete | `Delete(helper.go)` | definitions disappear and incoming call becomes unresolved; equals fresh build |
| rename | `Delete(added.go)` + `Upsert(renamed.go)` | old path/target is gone and incoming call retargets; equals fresh build |
| ignore | `dist/` and `node_modules/` | ignored source never enters the graph or source stamps |
| cache freshness | source and call-facts stamp changes | content-attested stamps change for each input |
| reconciliation | missing publication marker | refresh repairs the marker without changing the current graph |
| publication validity | source and graph tampering | validation rejects both; restored bytes validate |
| no-op refresh | unchanged current source | graph bytes remain identical |

Run the focused differential fixture with:

```sh
cargo test -p zg-codegraph --test codegraph_lifecycle_e2e
```

The fixture is deliberately independent of Graphify and external analysis
toolchains. It tests zvec's own full/incremental contract, ignore policy,
freshness inputs, persistence, and publication commit marker; it is not a
throughput or whole-repository accuracy claim.
