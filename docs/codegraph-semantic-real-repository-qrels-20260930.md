# Real-repository semantic call-facts qrels (2026-09-30)

The semantic qrels under
`tools/codegraph-semantic-benchmark/real-qrels/` are independently adjudicated
witness sets. They are not generated from graph output and are not
whole-repository accuracy claims. Each qrel pins the external checkout
revision, the source files used to adjudicate the calls, producer schema and
version, sidecar bytes, context fingerprint, and selected toolchain/context
inputs.

The validator re-hashes the producer's complete source and context file lists,
then reports one-vs-rest precision and recall for each normalized resolution
class. Static qrels additionally require the exact source declaration target.
Every class represented in the bounded witness sets had one-vs-rest precision
and recall of 1.00 in these runs.

## Results

The runs used read-only hard-linked staging roots so the external checkouts were
not modified. The producer artifacts were generated with the recorded local
toolchains and validated with the qrels in the same staging roots.

| Witness | Producer calls | Class accuracy | Static target accuracy | Result |
| --- | ---: | ---: | ---: | --- |
| `koku-operator` (Go) | 6 | 6/6 (1.00) | 3/3 (1.00) | passed |
| `praxis` (Rust) | 5 | 5/5 (1.00) | 2/2 (1.00) | passed |
| `kubernaut-ui-core` (TypeScript/TSX) | 5 | 5/5 (1.00) | 2/2 (1.00) | passed |
| `koku-python-common` (Python) | 5 | 5/5 (1.00) | 3/3 (1.00) | passed |

The evidence covers static local calls and bounded non-definite classes,
including Go interface dispatch, Rust trait dispatch, TypeScript callback and
function-value calls, and Python external calls. It validates producer
classification independently; it does not claim that every real-root sidecar
is accepted by the graph parser. Full-root overlay attempts also exercised the
consumer's conservative stale/malformed fallback: the first parser-locator
mismatch caused that language's overlay to be rejected rather than partially
committed.

## Reproduction

```sh
python3 tools/codegraph-semantic-benchmark/validate_callfacts_qrels.py \
  --root /path/to/staging-root \
  --revision-root /path/to/external-checkout \
  --qrels tools/codegraph-semantic-benchmark/real-qrels/<name>.json \
  --sidecar /path/to/staging-root/.zvec-grep/<sidecar>.json
```

The qrels and validator remain local-only evidence tooling. They do not add
Graphify, Pyright, rustc-dev, TypeScript, or Go analysis packages to zvec's
runtime, build, or CI dependency graph.
