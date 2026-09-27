# Issue #8: Code IR implementation plan

**Design:** [code-ir-design-issue-8.md](./code-ir-design-issue-8.md)

**Branch/worktree map:** [Code IR dependency and resume map](./code-ir-worktree-and-dependency-map.md).

**One-pass frontend evidence follow-up:** [source-backed signature and adjacent
documentation recheck](./code-ir-one-pass-metadata-results-20260926.md)
records the opt-in v1.6 milestone and remaining conformance/retrieval gates.

**Issue:** https://github.com/jordigilh/zvec-grep/issues/8

**Current implementation base (2026-09-26):** PRs #9–#12 were merged into
`integration/zvec-live-code-intelligence` at `acb3abc`. The historical
`spike/code-ir-design` checkout remains separate; continue new review units
from merged integration. The [metadata coverage assessment](./code-ir-metadata-coverage-results-20260926.md)
is the next opt-in gate; fork `main` has not received these PRs.

## Objective and sequence

Implement a **versioned, source-backed evidence contract** for Go, Python,
Rust and TypeScript. It must permit lexical/vector and structural projections
from the *same* snapshot while keeping exact source bytes as the answerable
evidence. Deliver in this order:

```text
freeze controls → schema/invariants → four-language frontends → conformance
  → opt-in snapshot publication → opt-in projections → paired evaluation
                                  ↘ optional SCIP semantic-enrichment pilot
```

The implementation is not complete merely because a JSON type exists or a
codegraph can be serialized. Each milestone below has an observable exit gate.
Keep the existing TypeScript and Rust lexical-only search paths working as the
default throughout. Retrieval-rank changes, graph expansion, and LLM context
format are independently gated, not automatic consequences of this IR.
The [design's SCIP interoperability section](./code-ir-design-issue-8.md#scip-interoperability-optional-enrichment-after-ir-conformance)
adds an **optional follow-up** after syntax-only IR conformance; it does not
block publication, projections or evaluation of the base IR.

## 0. Record the control and implementation seams

- Capture the exact checkout revision and existing TypeScript/Rust index versions,
  CLI flags and regression tests. Keep unmodified same-engine baseline binaries
  and source-selection/model configuration for later paired runs; base the
  experimental comparison on this checkout's `5f2c5e4` behavior, rather than
  comparing a candidate Rust engine to a TypeScript baseline. The older paired
  baseline protocol at Engram `40d4f58` documents the evaluation procedure,
  not a claim that those older arms are this new control.
- Follow the current data path before wiring anything: TypeScript
  `src/engine/extraction/code/extractor.ts` →
  `src/engine/pipeline/indexing/index.ts` → `src/engine/storage/zvec.ts`;
  Rust `rust/crates/zg-engine/src/extraction/code.rs` →
  `rust/crates/zg-engine/src/pipelines/indexing/` → workspace manifest in
  `rust/crates/zg-engine/src/workspace/`; graph sidecar in
  `rust/crates/zg-codegraph/src/lib.rs` and its MCP cache in
  `rust/crates/zg-transport-mcp/src/lib.rs`.
- Exit: a run manifest/README in this worktree identifies the frozen control,
  independently runnable tests, engine and fixture digests, and how to toggle
  the candidate *without* changing default search behavior. Do not edit the
  Engram fixture source, unit manifests, truth, queries or qrels.

## 1. Implement the IR v1 contract and validators

1. Make a single normative, machine-readable schema (e.g.
   `schemas/code-ir-v1.schema.json`) with the `Snapshot`, `File`, `Unit`,
   `SourceRef`, and `Fact` types specified in the design. Give it an independent
   `schema = zvec-grep.code-ir` and `schema_version = 1`. Rust and TypeScript
   representations must decode/validate the same serialized records; choose
   small internal modules or a dedicated `rust/crates/zg-code-ir` crate and
   `src/engine/code-ir/` as the code structure warrants. Avoid tying the schema
   to either language's parser types or to an MCP output format.
2. Document canonical encodings for snapshot/file/unit/fact IDs and version
   inputs. Namespace repository and scan root; hash the sorted selected file
   path/digest set and frontend/schema configuration. Use byte ranges and
   deterministic occurrence ordinals so overloads and coincident names cannot
   collapse. Cross-revision matching remains a separate feature.
3. Validate each source reference against the original file bytes: immutable
   SHA-256, `[start_byte,end_byte)` UTF-8 boundaries, 1-based lines/0-based
   **byte** columns, exact source slice and bounded file length. Reject a
   mismatch or expose a stale/fallback result, never cite synthetic lexical
   text. Validate unique IDs, parent and fact endpoint membership, language
   extensions, parse status, and the status/provenance invariants: name-only
   candidates in `candidate_ids`; `object_id` for syntactic containment or
   genuine type-resolved bindings only.
4. Versioned JSON round-trips must work in both runtimes, including future
   optional extension fields. Promote the four *illustrative* snippets in the
   design to complete serialized conformance examples (the design's compact
   JSON omits required production fields); keep the source and its digest next
   to each fixture.
- Exit: cross-runtime validation and deterministic serialization tests pass;
  corrupted hash/range, duplicate ID, unknown version, invalid relationship
  status, and stale file cases fail closed. No search or index behavior changes.

## 2. Build loss-aware frontends for all four languages

- **TypeScript runtime:** extract units *before* outline/window splitting in
  `src/engine/extraction/code/extractor.ts` and its language adapters. Convert
  `web-tree-sitter` JS string indexes to original-file UTF-8 byte offsets using
  a checked source map; the observed `// é 🚀\n` prefix gives parser index
  `8` but byte offset `11`. Keep fragment IDs and old return semantics in the
  default path. Map embedded Vue/Svelte `<script>` spans to the original bytes
  or report them opaque until round trips pass. If the existing text decoder
  replaced invalid UTF-8, mark the IR file opaque/failed instead of asserting
  a digest of a lossily decoded string.
- **Rust runtime:** derive the same schema from
  `rust/crates/zg-engine/src/extraction/code.rs` tree-sitter byte spans and the
  original read buffer; preserve the existing byte-slice validation. IR unit
  IDs are separate from content-derived `EntityId` and relative `FragmentId`.
- For **Go, Python, Rust and TypeScript** in both paths, define a small common
  file/module/type/function/method/value/opaque mapping while retaining
  subtype, syntax kind, receiver/decorator/trait/impl/overload and other
  language details as optional extensions. Keep distinct declarations even
  when names and qualified names coincide. Only keep signature/documentation
  on the canonical record when their own source spans are verified; otherwise
  put derived text into a projection.
- Emit syntactically observed `contains` and call-site facts with actual site
  spans. The Rust codegraph can supply candidate names and spans *only when*
  root/path/file digest agrees. Its `resolved: true` is a name heuristic:
  translate it to `name_candidate`, not `type_resolved`. Its import edges lack
  site ranges; skip those until anchored. Do not use deduplicated graph nodes
  as the sole source for overload/duplicate declaration inventory. Do not
  invent reads/writes/guards/implements/tests when extraction is unavailable.
- Unsupported syntax and parse errors must carry `opaque`/`partial` coverage
  with source-backed fallback windows; no missing edges may be interpreted as
  proof that none exist. Exclude facts touching error regions unless their
  spans and semantics are independently valid. Include generated-source
  provenance only when a real origin map exists.
- Exit: four-language tests assert exact source slices, kinds, parentage,
  duplicated names, and typed relation status. Exercise multibyte/astral
  Unicode, CRLF, multiline splits, closures, macros, decorators, overloads,
  generated files, unsupported extensions, parse errors and component scripts.
  The current extraction tests and `zg-codegraph` suite still pass.

## 3. Measure conformance before enabling retrieval

- Use Engram's independently frozen source-language fixtures and protocol at
  `../engram/benchmarks/semantic_search/` (38 Go / 33 Python / 39 Rust /
  33 TypeScript manifest units, eight queries per language). Build a *new*
  versioned IR-specific reference inventory with explicit byte spans and
  relation sites where the old manifests lack them (notably Go); do not alter
  adjudicated qrels or infer truth solely from candidate output.
- Produce one report per language **and per frontend**: manifest units matched
  by exact file/byte span/kind, all emitted unit/fact source round-trips,
  duplicates/overloads retained, call sites observed, and the breakdown of
  name-candidate/ambiguous/unresolved/type-resolved bindings. Count skipped
  syntax, failed files and missing relationship inventory explicitly; include
  Rust's six grade-0 module-declaration units. Attach provenance and missing
  cases, not only a percentage.
- Exit: 100% exact byte round-trips for *emitted* records; unsupported or
  ambiguous evidence is explicit; coverage against the independently labeled
  inventory is quantified and reviewed. A missing manifest unit is a recorded
  correctness gap, not a fabricated successful mapping. Keep IR shadow-only
  for any lane that cannot pass this gate.

## 3a. Optional SCIP semantic-enrichment pilot (non-blocking)

- After the syntax-backed IR passes phase 3, prototype a SCIP consumer in
  shadow mode, first for TypeScript, Python and Rust, then Go if the producer
  passes the same gate. Treat SCIP as optional **input** to the IR, not the
  canonical source model; absence of a SCIP producer must not prevent phases
  4–6 for the syntax-only path. See the [SCIP protocol](https://github.com/scip-code/scip/blob/main/scip.proto)
  and [design mapping](./code-ir-design-issue-8.md#scip-interoperability-optional-enrichment-after-ir-conformance).
- Record indexer name/version/arguments, root, dependency/toolchain context,
  revision/worktree, selected file set and its independently verified digests.
  SCIP itself provides no commit or file hash. Import semantic bindings only
  when the *whole* indexed source snapshot matches the active IR snapshot; a
  changed dependency can invalidate a binding even if its occurrence file did
  not change. Reject stale/unknown or cross-root joins.
- Convert `Document.position_encoding` and typed/legacy `Occurrence` ranges
  from 0-based UTF-8/UTF-16/UTF-32 positions to validated IR byte spans.
  Definition token ranges join an existing unit only after checking its
  declaration span (including optional `enclosing_range`). Map verified
  reference occurrences to anchored `references` facts; retain external SCIP
  symbol keys as provenance rather than replacing IR IDs. Map implementation
  relationships only if both endpoints and supporting definition site match.
  Rendered SCIP documentation/signatures are not source citations.
- Record whether each producer is semantic or heuristic for a given relation;
  only proven semantic bindings may use `type_resolved`. Do not promote
  name-only matches, infer calls from generic references, or infer general
  dataflow from read/write occurrence roles. Compare annotated definition,
  reference and implementation precision/coverage, source-span fidelity,
  ambiguity, freshness, generation cost and failures per language against
  syntax-only IR and trusted navigation tools. Evaluate SCIP-backed ranking
  only as a separate same-engine, same-language ablation.
- Exit: a documented opt-in adapter and shadow report if the measured producer
  satisfies these gates, or an explicit abstention report otherwise; neither
  result changes the default lexical path or the base IR completion criteria.

## 4. Persist and publish a consistent opt-in snapshot

- Introduce an explicit index option with `off` (default), `shadow`, and
  `projection` modes; expose it through the engines' existing CLI/API patterns
  rather than an implicit environment switch. `shadow` writes/inspects the
  sidecar but does not alter search. `projection` is allowed only after the
  preceding gates. Persist IR/version/frontend/source-selection metadata and
  source bytes or verifiable byte references separate from retrieval text.
- For a full build, read each source once, derive IR and all enabled projections
  from that buffer, validate digests/ranges, stage them as one generation, and
  atomically publish the active manifest last. Rust's rebuild staging in
  `rust/crates/zg-engine/src/workspace/build.rs` can be extended; TypeScript's
  `replaceFile` flow must not expose an IR/index cross-generation join. In
  shadow mode a lagging sidecar is acceptable only because search ignores it.
- For incremental add/modify/delete/rename, invalidate impacted units/facts,
  projections and incoming name-candidate edges, then publish a new manifest;
  compare incremental and fresh-full output on the same bytes. A file hash,
  root selection, IR schema, frontend/grammar, projection, resolver or model
  version mismatch must trigger the appropriate full rebuild, re-projection,
  re-resolution or re-embedding described in the design. Readers pin one
  active snapshot and refuse cross-snapshot joins. A crash midway leaves the
  prior published snapshot usable; stale/unsupported IR falls back to existing
  lexical/source behavior.
- Exit: integration tests cover unchanged files, partial failure, restart,
  cancellation, version mismatch, file rename, cross-file calls, index/IR
  mismatch, and concurrent reader during publication. Verify exact/regex,
  filters, freshness, and public result source are unchanged in default mode.

## 5. Derive projections without embedding ranking policy into IR

- Implement a deterministic projection adapter `IR unit/facts -> current
  combined lexical_text + current vector input + bounded fragments/group IDs`;
  keep aliases, name parts and outlines in this adapter, never in the canonical
  answerable source. Start by matching the existing TypeScript combined FTS
  field (`src/engine/extraction/vector-content.ts`, `storage/zvec.ts`) and
  Rust's current same-engine lexical behavior, not the previously rejected
  fielded/query-expansion or graph-recall experiments.
- Store `ir_snapshot_id`, projection version, embedding model identity and IR
  unit/source-ref linkage on derived records. Before returning any hit, verify
  that the active manifest, projection and file digest agree; source bytes are
  the evidence and generated projection text never appears as a citation.
  Preserve existing group-aware normalization, search trace and raw result
  ranks. No graph reranker is enabled by a data-format switch.
- Exit: deterministic projection fixtures and search/storage integration tests
  pass for both engines, with parity of unchanged candidate input/output where
  intended. Document any unavoidable projection difference before scoring it.

## 6. Evaluate, report, and gate enablement

- Rebuild unmodified controls and candidate indexes from fresh, separate
  directories on each of the four fixture lanes. Hold source/qrels/digests,
  `local/potion-code-16m-v2`, CPU/direct-hybrid flags, queries, scorer, model
  snapshot and limit fixed **within each engine/language pair**. Use the
  Engram [`MULTILANGUAGE_PROTOCOL.md`](../../engram/benchmarks/semantic_search/MULTILANGUAGE_PROTOCOL.md)
  and its fresh-output runner (or an equivalent reproducible runner if a new
  candidate opt-in flag needs plumbing). The linked Engram checkout is a
  reference, not a place for implementation edits in this worktree.
- Preserve raw backend ranks and normalized source-unit ranks. Report all
  eight per-query deltas and nDCG@10, MRR@10, recall@10 and precision@10 for
  every lane/engine, plus declaration/relation coverage and source round trips.
  Apply the per-language/engine gate: at least one aggregate metric improves
  with **no** decline in the other three; diagnose individual losses. Run
  adjudicated real-repository queries before a generalization claim. A neutral
  IR baseline may remain opt-in even if it is operationally useful; it is not a
  claimed search-quality win.
- Only after validated IR extraction and projections, rerun proposals 1–4 as
  **separate** ablations where supported; preserve baseline fallback, and
  leave a regressing lane disabled. Issue #7 owns the separate answer/citation/
  token/follow-up-read evaluation for any context renderer.
- Exit: reproducible commands and a provenance-rich result report in this worktree;
  no default changes without explicit passing evidence for that engine/language.
  If model/runtime availability blocks qeval, record the exact blocker and keep
  the candidate shadow/opt-in rather than claiming a passing lane.

## Verification and handoff checklist

- TypeScript: `npm ci` (if dependencies absent), `npm run build`, focused
  `node --test` extraction/index/search tests, then `npm run check` when the
  implementation changes public or index behavior.
- Rust: `cargo test -p zg-codegraph` (17 existing tests at plan time), tests for
  the chosen IR crate/modules and affected `zg-engine`/CLI/MCP components,
  `cargo fmt --check` and appropriate `cargo clippy ... -- -D warnings` after
  Rust changes. The local macOS build of the full `zg` CLI can require SDK
  libc++ include flags; see the Engram protocol instead of silently skipping
  a failed build.
- Inspect `git status`, generated files, and changed artifacts before handing
  off. Keep the proposal and this plan available for review in the dedicated
  worktree. Report milestones completed, tests/results by engine and language,
  remaining blockers and which feature mode is actually usable. Do not label
  coverage or ranking gains as measured without the corresponding artifacts.
