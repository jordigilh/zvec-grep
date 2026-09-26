# Factorial Code IR retrieval ablation plan (issue #8)

**Review slice:** branch `spike/code-ir-projection-factorial-ablation` on the
still-opt-in [PR #16](https://github.com/jordigilh/zvec-grep/pull/16) qeval.
Source snapshots and qrels are read-only; all indexes/artifacts are fresh and
outside the fork and Engram checkouts. No SCIP or default-search change.

## Before implementation: policy and tests

Keep the same compiled TypeScript engine, local CPU model, queries, frozen
source and Engram scorer for each language. Publish and **read-validate** both
v1 and v2 projections from exactly the same IR snapshot. Build a 2×2 experiment
from those published records, with baseline syntax search as a fifth arm:

| Factor | v1 base metadata + source text | v2 source-backed signature/doc text |
|---|---|---|
| v1 eligible units (file/opaque excluded from ranked qeval) | v1 eligible | v2 selected records **plus v1-only fields/attributes** |
| v2 selected units | v1 records filtered to v2 IDs | actual v2 |

This first slice tests only two differences: **standalone unit selection**
and **combined signature + documentation search text**. It does not isolate
signature from documentation, lexical from vector input, or window length.
Every shared unit must have the same source refs,
group/window IDs and ordered windows in v1 and v2; otherwise **abstain** instead
of confounding a text-policy comparison with changed window boundaries.
Underlying snapshot units/facts and cited source stay unchanged. The experiment
changes neither the search engine nor its fusion logic; query traces show route
ranks so later, separate text/window and fusion-threshold experiments can be
justified. `code-ir-v1` here means *eligible non-file, non-opaque records from
the validated v1 sidecar*, not the complete v1 sidecar or default search index.

Drive it with failing tests: rejecting mismatched snapshots/refs/window counts,
rejecting a missing v2 unit, ensuring v1-only units retain v1 text, shared units
use published v2 text, stable arm names and counts, and refusing duplicate or
misordered query IDs before scoring. The qeval runner's default two-arm path
must remain unchanged. Report per-query and aggregate scores **for every arm
per language**, not a pooled or cross-language average. Record artifact/source/
scorer/model hashes and the ablation implementation hash. Retain both arms
even if a factor fails; no tuning against the eight frozen queries.

Separate gates: Go's 38/38 independent source spans do not imply retrieval
fitness; other languages still need independently labelled exact byte sites.
Synthetic qrel line-overlap can credit containing class/impl/outline hits for
nested method labels. Independently judged real-repository retrieval and
negative relationship cases are still unavailable; do not enable any language
from this ablation alone.
