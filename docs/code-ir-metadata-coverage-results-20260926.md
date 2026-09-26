# Code IR source-metadata inventory (2026-09-26)

## Decision and method

PRs [#9–#12](./code-ir-worktree-and-dependency-map.md#review-stack-on-the-fork)
are merged into fork `integration/zvec-live-code-intelligence` at `acb3abc`.
Follow-up [PR #14](https://github.com/jordigilh/zvec-grep/pull/14) tests v1.6
signature/documentation **against independently
authored source-site labels**, instead of interpreting emitted metadata counts
as ground truth. It does not change the frontend, the retrieval projection, or
default search ranking.

The seven selected sites **per language** in
[`test/fixtures/code-ir-metadata-inventory-v1.json`](../test/fixtures/code-ir-metadata-inventory-v1.json)
were labelled by reading the frozen source files, not by copying extracted
metadata. The inventory pins the original manifest SHA-256, a sorted
manifest-path/source-hash digest, each selected file's SHA-256, a unique literal
UTF-8 signature/header or source anchor, and explicit documentation presence,
absence, or unsupported source text. The verifier first checks all these
against read-only source bytes, then extracts a fresh IR snapshot and checks
file identity, uniquely selected units, and exact byte offsets, source hashes,
and literal texts. Identically named Rust `struct` and `impl` units are
disambiguated using the author-labelled header span, not an IR-generated ID.

This is a **selected-case** assessment, not whole-fixture recall. The sample
intentionally spans types, methods, functions, value-field negatives, attached
comments, unsupported Python docstrings, and multiline headers. It was not
independently adjudicated by a third party and contains no retrieval qrels.

## Read-only frozen fixture replay

| Language | Selected source sites / represented IR units | Exact signatures / source-authored headers | Verified no-signature value units | Attached docs / labelled doc-bearing sites | Verified no-doc units | Site outside represented units or doc policy |
|---|---:|---:|---:|---:|---:|---|
| Go | 7 / 7 | 6 / 6 | 1 / 1 | 4 / 4 | 3 | None in this sample. |
| Python | 7 / 7 | 6 / 6 | 1 / 1 | 0 / 1 | 6 | `WorkflowState`'s **internal docstring** is source-visible but outside the v1.6 adjacent-comment policy; IR abstains. |
| Rust | 7 / 6 | 6 / 6 | 0 / 1 | 0 / 0 | 6 | `WorkflowState.ids` has no standalone IR unit in this frontend; its absent signature is **not** counted as verified. |
| TypeScript | 7 / 7 | 6 / 6 | 1 / 1 | 0 / 0 | 7 | None in this sample. |

An empty doc denominator for Rust or TypeScript is **not** a positive
documentation-coverage result. The `0/1` Python doc count measures the broader
source-visible docstring site, not a violation of the current policy's promise
to capture immediately adjacent comments. There are also authored synthetic
controls (outside these denominators) for multiline Go/Python/Rust/TypeScript
signatures with raw non-ASCII docs, Go CRLF bytes, Python decorators, Rust
attributes between doc and declaration, and TypeScript exports. A separate
bodyless multiline TypeScript declaration explicitly abstains instead of
publishing a truncated signature. Value initializers and detached comments are
negative controls. These controls establish supported/unsupported behavior,
not corpus prevalence.

## Reproduce and verify

Run `npm run build` then `node --test
test/unit/code-ir-metadata-inventory.test.mjs`. The test replays the external
fixtures if `../engram/benchmarks/semantic_search/fixtures` exists, or if
`CODE_IR_FROZEN_FIXTURES` points to that read-only root; it skips only the
external-corpus replay when the root is unavailable. All synthetic controls and
the inventory-shape check remain in the ordinary unit suite. For a single
language, run:

```sh
node scripts/replay_ir_metadata_inventory.mjs \
  --fixture /path/to/fixtures/go-workflow-discovery-v1 \
  --labels test/fixtures/code-ir-metadata-inventory-v1.json
```

Repeat with `python`, `rust`, and `typescript` in the fixture directory name;
each invocation reports its own denominator. A stale or changed manifest or
source set fails before extraction. The verifier reports missing IR units
separately from verified absent metadata, and rejects mismatching/ambiguous
source refs rather than inventing a passing label. Nothing is written to the
frozen fixtures.

## Limits and next gates

- This seven-site selection in each language cannot establish complete
  declaration coverage; expand independent labels (including more negative and
  unsupported source sites) before claiming whole-corpus metrics.
- Syntax metadata is distinct from source-attested relationships and retrieval
  relevance. Define a versioned opt-in retrieval-unit/text projection with TDD,
  retain fine-grained IR semantic endpoints, then run unchanged, same-engine
  qeval **per language**. Prior Go/Python gates failed; Rust/TypeScript had
  per-query losses. No default ranking change follows from this inventory.
- Independently adjudicated real-Kubernaut retrieval and negative relationship
  controls are still required before real-repository quality or enablement
  claims. Optional scip-go enrichment is not a blocker.
