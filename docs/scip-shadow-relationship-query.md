# Snapshot-bound Code IR relationship lookup (2026-09-25)

**Producer revision:** This report pins the original PR #298 custom binary.
The [review-response revision `c1926a3` was independently revalidated](./scip-go-pr298-revalidation-20260925.md):
its frozen Go Code IR factset is identical, it passes the same 20 positive and
three negative lookup controls, and it adds 367 valid declaration enclosures
on an immutable Kubernaut revision without changing primary sites. Use the
binary/index/shadow hashes of the specific revision being queried.

## Outcome

The compiler-like pipeline can keep semantic links as **validated relationships**
without copying their target names into every FTS/vector document. A read-only
offline query layer now opens a pinned SCIP-enriched Code IR shadow snapshot,
selects an exact unit (or exact reference byte site), and returns incoming and
outgoing **references** with original-source byte ranges, endpoint IDs and
producer provenance. A reference alone is not labeled a call, implementation,
or data-flow edge. The output status is `validated_shadow_candidate` rather
than a production type-resolution guarantee.

This is separate from the retrieval index: an agent can search for a candidate
unit, then use its Code IR ID or source path and symbol to inspect the bounded
relationship evidence. The prior [four-language retrieval qeval](./code-ir-multilanguage-qeval-results.md)
remains unchanged. Its flat SCIP-name embedding/FTS ablation failed, so this
lookup provides a different *query surface*, not a claim of ranking benefit.

## Measured Go evidence

On the frozen Go fixture, the read-only lookup used Code IR v1.5 snapshot
`38924ec8adb36ec929b2a9ac01f3ec09ce786fe8d355fad98e17a42507dc6be6`
and the custom scip-go PR #298 artifact with **117** strictly joined selected-
local reference facts. Replaying the independently source-labeled
[`go-reference-sites-v2.json`](./go-reference-sites-v2.json) found the labeled
local target for **20/20** sites, including same-file function and method
references, cross-file same-name methods and imported types.

Three additional source-selected negative controls
([`go-relationship-negative-sites-v1.json`](./go-relationship-negative-sites-v1.json))
abstained **3/3**: external `fmt.Sprintf`, external `unicode.IsLetter`, and
the `discovery` package import with multiple selected definition files. No
selected-local `object_id` was invented. This is a small authored control set,
**not** a precision measurement over all 321 raw Go references or all 117
strict facts. The compact machine-readable counts and producer digests are in
[`scip-shadow-relationship-query-summary.json`](./scip-shadow-relationship-query-summary.json).
An external reference is not necessarily unresolved; it can be outside the
attested file set.

For example, selecting `internal/selection/validator.go` /
`Validator::IsAllowed` returns the original source site `IsAllowed` at line 45
as one incoming reference from `Validator::Validate`, and five outgoing
references, including `Contains` at line 41 to
`internal/discovery/state.go::WorkflowState::Contains`. The field references
to `Validator::state` remain addressable semantic targets even though the
[retrieval projection policy](./code-ir-source-backed-projection-recheck.md)
does not rank the Go field as a standalone search hit.

The compact replay artifact is at
`/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/scip-go-v15-inventory-20260925/relationship-label-replay-v4.json`
on the evaluation host, SHA-256
`5ae73f26a5e36baf2097fffb9e2170ca32844c4f7d8366697834b1091d0ed85f`.
The positive/negative label inventories have SHA-256s
`4bb2a47ee2aa5cf0a9818abf90f716ab1fb3ec6c12e99d687a6d0bcb370b8efb` and
`faa944a07e9752558428caabcef7fef2c535a89bf66cd57b5a98814c34fea846`.

The same query contract was smoke-tested on the existing Python, Rust and
TypeScript v1.5 shadows. It returned source-backed incoming `validate` →
`is_allowed`/`isAllowed` and outgoing `is_allowed`/`isAllowed` → `contains`
references in each lane; the selected-local fact populations are respectively
**49**, **58**, and **47**. Those three checks demonstrate the query mechanics,
not whole-corpus relationship precision. Python/TypeScript shadows still depend
on their **version-pinned producer encoding controls** because their SCIP
documents declare position encoding 0; Rust declares UTF-8. The smoke probes
did not pin the producer executable hashes at query time.

## Validation and abstention contract

[`scripts/shadow_ir_relationships.mjs`](../scripts/shadow_ir_relationships.mjs)
refuses a query unless all of these agree before reading any fact:

1. The expected shadow artifact SHA-256 and its recorded SCIP index SHA-256.
   The optional executable SHA-256 pair pins the producer binary (used for
   Go); the shadow artifact itself does not prove which binary produced it.
2. The frozen manifest's complete selected-path set, root ID, language and
   file extraction statuses. Relative paths may not escape the root, and
   selected source files may not be symlinks.
3. The length-framed source-set digest used by the SCIP spike, per-file bytes
   and the real Code IR `validateSnapshot` source/ref checks. A changed target
   rejects the old snapshot even if the referring source is unchanged.
4. The separately pinned producer configuration digest (`go.mod`,
   `Cargo.toml`, `pyrightconfig.json` or `tsconfig.json`) and the exact compiled
   Code IR module/implementation hashes and frontend generation.

Lookup accepts only `references` with a local `object_id`, the experimental
`type_resolved` schema shape and `scip-shadow-v1` provenance. An exact site
with conflicting targets **abstains** rather than returning the first name
match; absent selected-local evidence returns `binding: null`. Unit searches
return per-direction counts, sorted source evidence and an explicit truncated
flag under a bounded limit. Synthetic tests exercised changed target with
unchanged caller, changed config, changed SCIP/shadow/module digest, deletion,
symlinked source, unsafe path selector and conflicting site targets.

The check is for **offline attested snapshots**. Full dependency/build-tag
attestation and an atomic proof of the producer's input read are not supplied
by SCIP or this query layer. Do not promote a dirty live worktree from a
post-hoc file hash alone. Runtime publication, MCP wiring, complete reference
precision and implementation relationship coverage remain separate gates.

## Reproduce Go lookup

This invocation uses the previously pinned fixture and custom scip-go build;
the source and producer inputs are read-only. Select by `--unit-id` from a Code
IR projection hit, or by an unambiguous `--path` plus `--symbol` as below.
`--site-path` plus `--site-start-byte` instead inspects one exact source site.

```sh
node scripts/query_shadow_relationships.mjs \
  --root /path/to/staged-root/go \
  --manifest /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1/manifest.json \
  --shadow /path/to/staged-root/shadow-ir-go-v15.json \
  --scip-index /path/to/staged-root/go/index.scip \
  --config go.mod \
  --shadow-sha256 7697906e0355c193e78c111bc75afe453926ece6cd53a74b0099c4f8fde0d30a \
  --source-set-sha256 13ca96abdeb8ef919a746f1ec1afb3ae5b6d59e8b0282d98109bc2b6a8c4f53d \
  --config-sha256 c648eda40444b75722e05f9db739a79109464100d982db9265a786105bc4e6c9 \
  --module /path/to/zvec-grep-code-ir-design/dist/engine/code-ir/index.js \
  --module-sha256 a6c903b199235648f580e7abfdfc1106d52afd2f742b504bef7dc9b33a438f42 \
  --implementation-sha256 1f207a060556996c6b8043247b3235c8690763cc7169aef8e4d4b2f968b23130 \
  --producer-binary /path/to/custom/scip-go-spike \
  --producer-binary-sha256 eb09bc8ca1315c474308b9f6ca356f4ec744b45f9986519d65319c825b214e0e \
  --path internal/selection/validator.go --symbol Validator::IsAllowed --limit 5
```

Use the same attestation options with `scripts/replay_go_relationship_labels.mjs`
and `--labels docs/go-reference-sites-v2.json --negatives
docs/go-relationship-negative-sites-v1.json` to recheck the small independent
truth inventory. Its output is created only when an optional new `--output`
path is supplied.
