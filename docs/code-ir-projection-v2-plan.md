# Code IR v2 opt-in retrieval policy plan (issue #8)

**Review:** [PR #15](https://github.com/jordigilh/zvec-grep/pull/15) branches
from metadata coverage [PR #14](https://github.com/jordigilh/zvec-grep/pull/14), itself based on the
merged integration tip `acb3abc`. This is the **next review unit**, not a
change to the default index. Frozen source/manifests/truth/qrels stay read-only.

## Contract before implementation

1. Preserve the existing `projection-v1` output and its implicit all-unit
   selection by default. `off` and `shadow` remain unaffected. Only an explicit
   `--projection-policy source-metadata-entity-v1` with `--mode projection`
   selects **projection format v2**; its policy ID appears in the active
   manifest and payload and has a distinct on-disk filename. Readback rejects
   version/policy/model mismatches and changed source refs.
2. In v2, exclude `file` and `opaque` source units, Go `struct_field` and Python
   `class_attribute` values **from standalone ranked projection records only**.
   Retain all units and facts in the same snapshot as semantic endpoints. Rust
   and TypeScript declaration units stay eligible; do not infer a relationship
   from text or rewrite source bytes.
3. Build source-backed `signature:` and `doc:` lines only from already validated
   v1.6 IR `SourceRef` slices, separate from answerable windows. Put the same
   bounded metadata on the FTS and vector input routes; keep original raw bytes
   and window refs as the only citations. Bound text to the existing 3,600-char
   projection limit, preserving UTF-8/CRLF and deterministic window coverage.
4. Drive implementation with failing controls: four-language v2 unit
   selection, unchanged v1 output, exact source-backed text, no fabricated
   metadata, opt-in CLI validation, Unicode-window round trips, immutable
   generation identity, and readback rejection of tampering/stale policy.

## Syntax-only recheck (no retrieval qeval)

The v2 candidate is published only when the explicit policy is selected.
`projection-v1` files remain intact and can be reactivated on the same source
snapshot. `projection-v2` carries version `2` and the policy ID in both the
manifest and payload; readback re-derives v2 records from the validated snapshot
and original source bytes, rejecting tampered text, refs, versions, and policy.

The separate, read-only frozen fixture replays produced:

| Language | IR units retained | v2 standalone ranked units | Value units retained only as semantic endpoints | Source-backed signatures / docs in IR |
|---|---:|---:|---:|---:|
| Go | 68 | 38 | 20 struct fields | 38 / 5 |
| Python | 53 | 33 | 12 class attributes | 33 / 0 |
| Rust | 63 | 49 | 0 under this policy | 36 / 0 |
| TypeScript | 46 | 38 | 0 under this policy | 33 / 0 |

These are **emitted/selected unit counts**, not independently judged whole-
fixture metadata coverage or relevance scores. For the smaller source-authored
signature/doc denominator use the [PR #14 metadata inventory](./code-ir-metadata-coverage-results-20260926.md).
Synthetic tests also verify same-snapshot v1→v2→v1 publication, UTF-8/CRLF
source-window coverage, bounded metadata on both FTS/vector routes, and
readback rejection of a fake `doc:` prefix. `file`/`opaque` units have no
standalone v2 records, so unsupported files need explicit fallback review
before any enablement. The v1/default search path does not consume this
sidecar; no ranking or relevance claim is made here.

After `npm run build`, one opt-in reproduction command per frozen lane is:

```sh
node scripts/code-ir-snapshot.mjs --mode projection \
  --projection-policy source-metadata-entity-v1 \
  --manifest /path/to/fixtures/go-workflow-discovery-v1/manifest.json \
  --output /path/to/fresh/output-dir
```

Use separate fresh output directories for Python, Rust and TypeScript. Omitting
`--projection-policy` continues to emit v1; the sidecar never changes existing
search storage. The external source checkout is read-only. The optional frozen
unit test replays those lanes when `CODE_IR_FROZEN_FIXTURES` points to the
fixture root (or it exists at the sibling Engram path).

On this branch, `npm run build`, `npm run lint`, `npm run format:check`,
`npm run typecheck`, `npm run code-ir:cross-runtime` and the unit suite passed
(313 passed, one skipped). The broader root suite was not rerun; no paired
qeval was performed. These verification results are **not** a retrieval
enablement gate.

## Gate after this PR

Use fresh, separate same-engine indexes per language and the unchanged frozen
qrels/scorer/model to compare the old syntax control and v2; publish per-query
and aggregate results. Historical Go/Python regressions and Rust/TypeScript
query-level losses mean v2 **must not be enabled by default** simply because
this contract passes. Keep qeval/real-Kubernaut judgments separate from
source-map and metadata correctness. Optional semantic producers do not block
this syntax-only policy.
