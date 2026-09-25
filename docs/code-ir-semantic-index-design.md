# Source-mapped semantic indexing: intent and planned design

**Status:** Proposed architecture for [Code IR issue #8](https://github.com/jordigilh/zvec-grep/issues/8),
checkpointed 2026-09-25. The [work-status page](./code-ir-scip-work-status.md)
tracks current implementation, upstream dependencies and the next action.
The owning team's [Code IR v1 contract](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-design/docs/code-ir-design-issue-8.md)
and [implementation plan](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-design/docs/code-ir-implementation-plan-issue-8.md)
specify the canonical schema and frontend milestones. This document describes
the *intended indexing and query system built on that contract*, informed by
the separate SCIP/projection experiments. It proposes integration behavior;
it does not assert that a production pipeline already exists.

## Intent and success criteria

Build a local-first, compiler-style code index for Go, Python, Rust and
TypeScript that can answer two different questions from the **same exact source
snapshot**:

1. **Where is useful source?** Discover answerable declarations and bounded
   source windows via lexical/vector search, with exact original bytes and
   locations as the evidence.
2. **What is this source demonstrably connected to?** Given a discovered
   unit or exact byte site, inspect validated incoming/outgoing relationships
   with source sites, endpoints, strength and provenance; abstain if identity or
   resolution is uncertain.

Semantic connections belong in an evidence index, not in every embedding or
FTS field. Search quality and relationship correctness have independent
acceptance gates. The existing source/exact/lexical search remains usable when
a parser or optional semantic producer is absent or stale.

**Success** means that a reader can trace a hit and every returned local edge
to an immutable file, validated UTF-8 byte slice, one snapshot and the specific
producer decision that supports it. It does not mean that every symbol has a
target, every reference is a call, or every language has identical coverage.

## Ownership and boundaries

- The owning Code IR implementation defines and validates versioned `Snapshot`,
  `File`, `Unit`, `SourceRef` and `Fact` records, per-language syntax frontends,
  publication and source-backed fallback. This design **consumes that one
  contract** rather than defining a competing schema. Within-snapshot IDs are
  authoritative; names/SCIP symbols do not supply cross-revision identity.
- The optional SCIP adapter supplies candidate definitions and reference
  bindings from a producer such as [scip-go PR #298](https://github.com/scip-code/scip-go/pull/298).
  A versioned LSP adapter may be another producer under its own position and
  freshness rules; it never fills in an unspecified SCIP position encoding.
- Retrieval projections, relation lookup and any eventual answer/context
  renderer consume the validated snapshot. They have separate versions and
  rollout gates. [Issue #7](https://github.com/jordigilh/zvec-grep/issues/7)
  owns downstream answer/citation and token-budget evaluation.

## Proposed data path

```text
selected original source bytes + root/config/frontend identity
                │
                ▼  parse once per selected file; verify original byte slices
  Code IR snapshot: files, units, source refs, observed syntax facts
                │
       ┌────────┼─────────────────────────────────┐
       │        │                                 │
       │        ▼                                 ▼
       │  versioned retrieval projection      source renderer
       │  (FTS, vector, bounded windows)      (original bytes only)
       │
       ▼ optional frozen producer + its dependency/build snapshot
  SCIP/LSP position conversion, full-scope attestation, exact definition join
       │
       ▼
  validated local facts + external locators + explicit abstentions
       │
       ▼
  bounded relationship lookup by IR unit ID or exact source site
```

**Syntax first.** A frontend reads each selected file's bytes once, parses
them, and emits units for supported declarations with byte-verified identifier,
body, signature and adjacent documentation refs where available. It preserves
receiver/field/attribute and language-specific syntax in the owning IR's kinds,
subtypes and extensions. A field or class attribute can remain a semantic
target even when it is not a standalone ranked search hit. Parse errors or
unsupported constructs yield explicit partial/opaque coverage and source
fallback instead of invented units or links. Signature/comment capture during
the same read/parse replaces this spike's second-parser metadata experiment.

**Semantic enrichment.** In an isolated immutable input root, build an
index with an identified executable, arguments, source selection, dependency
closure and build configuration. Decode only a declared position encoding or
a narrowly version-pinned producer encoding verified by real Unicode/CRLF
controls. Map occurrence positions to original UTF-8 half-open bytes, check
identifier token and optional enclosing declaration separately, and join a
definition only to one existing source-, kind- and scope-compatible IR unit.
A verified reference site may become a `references` fact from its smallest
containing unit to that local definition. A source outside the attested set
remains an external locator; multiple plausible local targets abstain from a
single `object_id`. Producer roles alone do not prove `calls`, `implements`,
read/write flow or tests. A stronger fact kind requires independently
validated supporting sites/endpoints and producer semantics for that kind.

**Resolution strength.** Preserve the owning IR's explicit status and
provenance (`observed`, `name_candidate`, `ambiguous`, `unresolved`, or
`type_resolved` when justified). The current strict SCIP shadow uses the
schema-valid fact shape but is exposed as a `validated_shadow_candidate`
until producer-specific resolution and negative-case evidence warrant stronger
runtime claims. A name-matched codegraph edge is a candidate even if a
sidecar calls it resolved. Keep the producer symbol as provenance, not an IR
object ID or source citation.

**Two read surfaces.** Search returns a projected hit containing the active
`snapshot_id`, `unit_id`, verified `SourceRef` and projection version; rendered
text comes from that ref. A separate bounded lookup accepts that unit ID or an
exact byte site plus the same snapshot, direction (`incoming`/`outgoing`) and
limit. It returns typed facts with source sites, local endpoint refs, producer
identity, status and truncation/abstention information. It rejects an ambiguous
site, stale target, mismatched configuration or mixed generations rather than
guessing. The [read-only prototype](./scip-shadow-relationship-query.md)
establishes the query shape; CLI/MCP/API wiring and runtime publication are
future milestones. Flat SCIP target names are not inserted into every hit's
FTS/vector input by default: the [measured four-language ablation](./code-ir-multilanguage-qeval-results.md)
failed the retrieval gate in every lane.

## Freshness, publication and fallback

Three identities must be explicit and linked, without treating a producer's
`Metadata.project_root` or tool version string as a source digest:

| Artifact | Required binding |
|---|---|
| Base IR snapshot | Schema/frontend versions, selected root/path list, exact file-byte digests, source maps and parser options. |
| Semantic generation | Base IR snapshot plus complete indexed source/dependency/build inputs, producer executable/args/version, adapter version and SCIP/LSP artifact digest. |
| Retrieval projection | Base IR snapshot, source-backed unit-selection/text policy, projection version and embedding model identity; source evidence stays in the IR/file store. |

For offline builds, freeze the entire producer-eligible source and relevant
dependencies/configuration **before** the producer reads them. Validate the
same immutable inputs after indexing and before joining. A post-hoc hash of a
mutable checkout alone cannot prove which bytes the producer saw. If complete
dependency attestation is unavailable, restrict the result to an offline
diagnostic shadow or abstain from the affected binding. A dirty checkout can
form a new content-addressed snapshot; it cannot reuse a clean-commit index.
For live changes, invalidate the old generation when any selected or resolving
input changes, including an unchanged referring file with a changed target.

When a production generation is built, derive IR and projection candidates
from the same source read, stage them with their versions, validate refs and
publish **one active manifest last**. Readers pin that manifest for a request.
An optional semantic generation joins only when its full identity matches;
it can lag without blocking syntax/search. Crash, unsupported source, missing
producer or stale hash returns explicit abstention/fallback, never a link across
snapshots. Incremental add/modify/delete/rename must invalidate impacted units,
projections and incoming edges; compare its output with a fresh full build on
identical bytes. Schema, parser, projection and resolver version changes have
separately defined rebuild/reprojection/re-resolution triggers in the owning
[implementation plan](https://github.com/jordigilh/zvec-grep/blob/spike/code-ir-design/docs/code-ir-implementation-plan-issue-8.md).

## Phased delivery and decision gates

| Stage | Proposed deliverable | Exit gate |
|---|---|---|
| 1. Syntax/source parity | One-pass source-backed units, signatures/docs when available, explicit opaque/partial coverage, and versioned retrieval-unit policy. | Exact byte round trips on **every emitted** record; independently inventoried declaration/kind coverage reported for all four languages; current default path unaffected. |
| 2. Producer shadow | Per-producer position/encoding controls, whole-input attestation, strict unit join and bounded source-backed reference lookup. Go PR revisions remain separate artifacts. | Independently authored positive, negative, ambiguous and external cases; per-kind coverage and false-link/abstention counts, stale/dirty/rename/dependency tests. Keep unproven relations shadow-only. |
| 3. Snapshot-bound read API | Stage/publish one opt-in IR generation; pin lookup to it; expose bounded local relationships only after the relevant producer gate. | Restart/crash, partial write, concurrent reader, stale target/config and incremental-vs-full tests; exact source refs and lexical fallback always available. |
| 4. Retrieval projection | Same-engine paired qevals with source metadata and explicit unit selection; search first, relationships on request. | Per-language nDCG/MRR/recall/precision@10: at least one aggregate improvement, no decline in the other three, and review all per-query losses; independently judged real-repository qrels before generalization. |
| 5. Optional downstream context | Render source-backed units and bounded facts only after earlier gates. | Issue #7's citation, sufficiency, token and follow-up-read evaluation; no implied graph rerank. |

Stages 2 and 4 are independent: a trustworthy navigation shadow does not
automatically improve ranking, and an IR-only retrieval projection does not
depend on SCIP merging. Any lane that fails a gate remains opt-in or in shadow.
The [work-status checkpoint](./code-ir-scip-work-status.md) records actual
progress: Go's 117 strictly joined selected-local references and 20/20 chosen
positive sites plus 3/3 abstentions; the 1,070-document Kubernaut run checks
source-map coverage only. Go/Python currently fail the frozen retrieval gate;
Rust/TypeScript pass aggregate with query-level losses. These measurements do
not fulfill the whole-repository relation and retrieval gates above.

## Decisions requiring evidence

1. **Producer promotion:** Which versions and construct kinds support a
   justified `type_resolved` status? Revalidate any later scip-go PR #298 head
   on unchanged source, compare every primary site and changed enclosing
   range, and check new generic/local/grouped declaration cases. Maintainer
   agreement on the [proposed scope](https://github.com/scip-code/scip-go/pull/298#issuecomment-5839828937)
   and passing CI are separate from source/semantic acceptance here.
2. **Snapshot closure for real repositories:** What exact Go build tags,
   generated inputs, dependencies and toolchain outputs affect producer
   resolution, and how are they frozen? Apply the equivalent question to each
   language rather than reusing Go's attestation policy unexamined.
3. **Unit/relationship UX:** Which source units should be independent search
   hits, and what bounded lookup controls make navigation useful without noisy
   outputs? Measure on frozen and independently judged real-repository cases;
   keep the canonical IR's fine-grained units available as endpoints.
4. **Real-repo ground truth:** Author Kubernaut relation and retrieval labels
   independently of candidates, including negatives and ambiguity, before
   claiming whole-corpus precision or a quality win. Keep each language's
   fixture, qrels, metrics and producer evidence independently versioned.
