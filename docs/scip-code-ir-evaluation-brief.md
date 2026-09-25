# SCIP → Code IR evaluation spike

**Purpose:** Determine with reproducible evidence whether SCIP indexers can
enrich [Code IR issue #8](https://github.com/jordigilh/zvec-grep/issues/8) with
accurate source-mapped definitions, references and implementations while
preserving snapshot identity and the existing syntax/lexical fallback.

**Checkout:** This isolated `spike/code-ir-scip-evaluation` worktree at
`5f2c5e4` is for the experiment. The concurrently changing
[`Code IR design`](../../zvec-grep-code-ir-design/docs/code-ir-design-issue-8.md#scip-interoperability-optional-enrichment-after-ir-conformance)
and [implementation plan](../../zvec-grep-code-ir-design/docs/code-ir-implementation-plan-issue-8.md#3a-optional-scip-semantic-enrichment-pilot-non-blocking)
in the sibling worktree are read-only references; do not edit their worktree.
Engram's [semantic-code-intelligence spike](../../engram/docs/SEMANTIC_CODE_INTELLIGENCE.md#scip-direction)
frames SCIP as a snapshot/provenance capability; its four-language qeval
[protocol](../../engram/benchmarks/semantic_search/MULTILANGUAGE_PROTOCOL.md)
supplies frozen, independent source/unit fixtures (38 Go, 33 Python, 39 Rust,
33 TypeScript units). Do not modify those fixture sources, manifests, qrels or
truth; stage copies in an isolated temporary area if the indexers need setup.

## Questions and measurements

1. **Actual producer support.** On the four fixture languages, identify working
   local producer versions and exact invocations: `scip-typescript`,
   `scip-python`/Pyright, `rust-analyzer scip`, and `scip-go` where possible.
   Record runtime/toolchain/dependency setup, output digest, file count, cost
   (time and resource usage if measurable), failures and which files were not
   indexed. Do not assume that a listed indexer is installed or that a
   producer is compiler-precise for every kind of symbol. Prioritize
   TypeScript/Python/Rust, then Go; report all four explicitly.
2. **Source fidelity.** Decode actual `index.scip` artifacts, map
   `Metadata.project_root` and `Document.relative_path` to the staged root,
   and interpret each document's `position_encoding`. Convert 0-based
   UTF-8/UTF-16/UTF-32 character positions and both typed/legacy occurrence
   ranges to `[start_byte,end_byte)` over *original* UTF-8 source, with
   1-based lines/0-based byte columns. Verify each mapped slice and report
   invalid/empty/missing spans by producer. Inspect definition token range
   versus optional enclosing declaration range; do not equate them. Include
   a multibyte/astral Unicode and CRLF control distinct from the frozen
   fixture data.
3. **Semantic precision and coverage.** For each lane, measure definitions
   matched to independently labeled source units (supplement existing manifests
   with separately authored exact spans where missing), references with
   verified source sites, implementation/type relationships and external
   symbols. Distinguish correct bindings, false links, ambiguous, unresolved
   and unsupported facts. Check producer provenance before proposing an IR
   `type_resolved` status. A reference is not necessarily a call; SCIP roles
   for reads/writes are not proof of full data flow. Compare representative
   exact links with available language-service/Serena results rather than
   treating a SCIP symbol string as truth.
4. **Snapshot and worktree semantics.** SCIP's protocol metadata includes
   project root/tool information but no source-file SHA-256 or indexed commit.
   Capture an independent attestation for the *entire selected file set* plus
   producer/version/config and git/worktree state. Exercise unchanged repeat
   indexing and a dirty edit, deletion/rename and changed cross-file target
   in *staged copies*, not frozen fixtures. Demonstrate that an adapter rejects
   stale cross-file bindings even when the referring file itself is unchanged.
   Report whether a fresh dirty-worktree run can be attested at all.
5. **IR fit and value.** Provide a concrete, minimal mapping from real SCIP
   `Document`/`Occurrence`/`SymbolInformation`/`Relationship` to the current
   Code IR schema: local unit binding, anchored `references`/`implements`
   facts, external locators, producer-specific provenance, source digest and
   resolution status. Report gaps (missing enclosing ranges, absent source
   bytes, unsupported syntax, incomplete producer scope) without fabricating
   facts. An optional shadow-mode import is the target; ranking is a separate
   same-engine, same-language ablation only if extraction/source fidelity
   passes. No production-default integration is part of this spike.

## Deliverables and decision gate

- A reproducible report `docs/scip-code-ir-spike-results.md` with a per-language
  evidence table, source/producer/protocol versions, commands, denominator and
  numerator for each coverage metric, failure/ambiguity cases, timing/resource
  observations, and exact artifact digests/paths. Keep large generated SCIP
  binaries out of git; retain compact verifiable output and a reproduction
  script in this worktree if needed.
- A small executable range/snapshot-attestation prototype and meaningful
  fixtures/tests if the format can be parsed in the environment; otherwise
  document the exact dependency or build blocker with commands/output rather
  than claiming a measured result. The prototype must reject unknown encoding,
  mismatched file content and unverified cross-file snapshots.
- A recommendation for each language: trustworthy anchored facts now,
  shadow-only candidate, or unsupported, with specific evidence and remaining
  work. Keep the syntax-backed IR and existing lexical path available regardless
  of producer success. SCIP is useful only if it improves relation correctness
  or coverage without weakening source attribution or freshness.

Use the [SCIP protocol](https://github.com/scip-code/scip/blob/main/scip.proto)
as the schema authority (including per-document position encoding and optional
enclosing ranges). Cite source files and real output samples in the report.
