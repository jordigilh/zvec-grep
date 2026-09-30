# Real-repository codegraph witness qrels (2026-09-30)

The local-only qrels under
`tools/codegraph-relations-benchmark/real-qrels/` are independent witness sets,
not whole-repository accuracy claims. Each file pins the external checkout's
Git revision and SHA-256 digests for the source files used to adjudicate the
relations. The labels were selected from source responsibilities and reviewed
against the generated graph; they were not copied from graph output.

## Validation command

Build `zg`, generate a plain JSON graph for each external checkout, then run:

```sh
python3 tools/codegraph-relations-benchmark/validate_real_qrels.py \
  --root /path/to/checkout \
  --qrels tools/codegraph-relations-benchmark/real-qrels/<name>.json \
  --graph /path/to/codegraph-v2.json \
  --zg rust/target/release/zg
```

The validator checks the pinned source files, positive and negative resolved
relations, and manually labeled same/different community pairs through the
public `communities` query. It is deliberately outside runtime, build, and CI
dependencies.

## Same-host results

The following runs used the checked-out revisions in the qrels on the same
local macOS host. Graph sizes are included to make the evidence auditable; the
timings are local process observations, not cross-machine performance claims.

| Witness | Revision | Graph | Positive / negative qrels | Community pairs | Result |
| --- | --- | ---: | ---: | ---: | --- |
| `praxis-filter` (`crates/filter`) | `932098a3` | 184 files / 6,097 nodes / 57,400 edges | 8 / 1 | 1 same, 1 different | passed |
| `koku-operator` | `e77bf79d` | 167 files / 2,260 nodes / 26,755 edges | 7 / 1 | 1 same, 1 different | passed |
| `kubernaut-console` (`packages/ui-core`) | `a5755470` | 129 files / 1,531 nodes / 14,835 edges | 7 / 1 | 1 same, 1 different | passed |

The witness relations cover syntax-backed containment, local imports and
re-exports, calls, references, tests, and implementation edges. Negative
qrels cover relations that the source does not establish. Community qrels use
pairwise constraints rather than pretending that a small manually selected
subset is a complete partition.

The current controlled graph-generation observations were 13.121 s, 3.152 s,
and 0.912 s respectively in the order shown above. Throughput work remains
isolated to issue #25; these correctness runs do not authorize a performance
claim.
