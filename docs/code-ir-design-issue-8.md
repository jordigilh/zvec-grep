# Code IR proposal for issue #8

**Recommendation:** Introduce an opt-in, versioned, source-mapped *evidence IR*
between language frontends and retrieval projections. Start with files,
declarations, exact source spans, and explicitly qualified syntactic call
observations. Retain the existing lexical/vector pipeline as the control and
fallback; do not enable graph ranking or new context output as part of IR v1.
Allow a later, opt-in SCIP import to enrich this IR with independently verified
semantic definitions/references/implementations; SCIP is a producer input, not
the IR's source or snapshot authority.

**Confidence:** 92% that this is a feasible *contract and staged implementation
plan* for the four investigated languages, based on the existing extractors,
codegraph, and paired evaluations below. This is a design judgment, **not** a
measured probability of retrieval improvement or a claim of production-ready
extraction coverage. Neither four-language conformance nor improved ranking has
yet been demonstrated for this IR. The optional SCIP importer is a proposed
enrichment path; its accuracy and operational value have not been measured.

## Verified constraints

| Observation at zvec-grep `5f2c5e4` / Engram `40d4f58` | Consequence |
| --- | --- |
| The TypeScript path produces AST symbol fragments with scope/signature/docs, but its IDs are `sha256(fileId, traversalIndex)` and large declarations become an outline plus overlapping fragments ([extractor](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/extraction/code/extractor.ts#L60-L108), [IDs](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/extraction/ids.ts#L1-L5)). | A canonical declaration needs its own identity and source range; fragments/outlines are projections, not declarations or evidence by themselves. |
| The TypeScript extractor stores parser `startIndex`/`endIndex` as `startOffset`/`endOffset`, and slices JS strings with these indexes ([extractor](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/extraction/code/extractor.ts#L341-L351), [window slicing](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/extraction/code/extractor.ts#L453-L476)). A local `web-tree-sitter` probe with `// é 🚀\nfunction add()...` produced `startIndex=8`, JS string index `8`, and UTF-8 byte offset `11`. | IR must convert JS/parser character coordinates into **original-file UTF-8 byte offsets**; never relabel an existing `startOffset` as a byte offset. Test astral and multibyte text, CRLF, large splits, and embedded `<script>` remapping. |
| The Rust extractor already uses tree-sitter byte spans and checks that entity content equals its source slice ([extractor](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-engine/src/extraction/code.rs#L184-L226), [test helper](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-engine/src/extraction/code.rs#L409-L454)). Rust entity IDs depend on **content and source range** ([entity](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-engine/src/domain/entity.rs#L11-L22)). | Reuse the byte-based validation pattern, but do not equate Rust/TypeScript retrieval IDs or graph IDs with IR IDs. |
| The separate codegraph supports Go, Rust, TypeScript/TSX and Python, records file SHA-256, definitions and call-site ranges, and can update changed files ([schema](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L69-L179), [update](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L296-L350)). Its calls are matched by simple name with same-file preference and then marked `resolved`; import edges have no source range ([resolver](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L628-L689), [imports](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L536-L557)). | Treat even `resolved: true` call edges as **name-based candidates**, never type-resolved facts. Preserve the observed call site independently of its guessed target. Defer importing edges without a source anchor until they can be mapped. |
| [SCIP's protocol](https://github.com/scip-code/scip/blob/main/scip.proto) provides per-document language, relative path, position encoding, source occurrences and symbol metadata/relationships. `Document.text` and `Occurrence.enclosing_range` are optional; SCIP metadata carries project root and producer information but no content digest or commit. The protocol permits both compiler-backed and heuristic indexers. | SCIP can enrich symbol bindings only through a snapshot-verified adapter; it cannot replace the source-map, provenance, fallback, or resolution-strength rules of this IR. |
| Codegraph IDs hash `(kind, path, qualifiedName)` and graph nodes are deduplicated by ID ([IDs](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L1415-L1438), [dedupe](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L605-L626)). | Overloads, duplicate declarations, and anonymous constructs require span/occurrence-based within-snapshot IDs; names alone are not an identity contract. |
| The TypeScript lexical projection stores combined `lexical_text` separately from returned source ([projection](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/extraction/vector-content.ts#L29-L39), [storage](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/src/engine/storage/zvec.ts#L70-L72)). Rust has an atomic *full rebuild* generation switch; the graph is a separately written sidecar ([build](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-engine/src/workspace/build.rs#L112-L166), [graph write](https://github.com/jordigilh/zvec-grep/blob/5f2c5e4/rust/crates/zg-codegraph/src/lib.rs#L806-L822)). | Keep generated text out of evidence records. Add an explicit publication key for IR and all projections; existing full-rebuild atomicity does not by itself make the independent graph and incremental index a single snapshot. |
| In paired, same-engine eight-query synthetic lanes, both lexical changes improve all four aggregates on Go and Python, but each loses recall on TypeScript; the TypeScript change also loses three aggregates on Rust ([paired results](https://github.com/jordigilh/engram/blob/40d4f58/benchmarks/semantic_search/MULTILANGUAGE_PAIRED_RESULTS.md#L11-L41)). Proposals 1, 2 and 4 regressed at least one Go metric; Proposal 3 was neutral ([issue #8](https://github.com/jordigilh/zvec-grep/issues/8), [issue #6](https://github.com/jordigilh/zvec-grep/issues/6)). | IR correctness and search quality need independent gates. A Go win, macro average, or one engine's score cannot authorize a global rollout. |

`cargo test -p zg-codegraph` passed all 17 unit tests in this dedicated worktree;
those tests demonstrate existing sidecar behavior, not the proposed IR.

## IR v1 contract

One immutable `Snapshot` contains the following *typed records*, serialized
with a separate `schema: "zvec-grep.code-ir", schema_version: 1`. The schema
version is independent of the search-index and `codegraph-v1` versions.

```text
Snapshot {
  repository_id, root_set, revision?: string,              // revision is advisory
  snapshot_id, schema_version, frontend_versions: {language: version},
  files: File[], units: Unit[], facts: Fact[]
}
File {
  file_id, root_id, relative_path, language, sha256, byte_length,
  extraction: {status: complete|partial|opaque|failed, reason?, frontend_version}
}
SourceRef {
  file_id, sha256, start_byte, end_byte,                  // [start, end) in original bytes
  start: {line, column_byte}, end: {line, column_byte}  // 1-based lines, 0-based byte columns
}
Unit {
  id, kind: file|module|type|function|method|value|opaque,
  subtype?, name?, qualified_name?, parent_id?, scope_id?,
  signature?: {text, source: SourceRef},
  documentation?: {text, source: SourceRef},
  source: SourceRef, origin: {language, frontend, syntax_kind},
  extensions?: {language, data}
}
Fact {
  id, kind: contains|calls|references|reads|writes|returns|throws|implements|tests|guards|imports,
  subject_id, object_id?, target_spelling?, candidate_ids?: string[],
  site: SourceRef, status: observed|name_candidate|ambiguous|type_resolved|unresolved,
  provenance: {frontend, resolver?, resolver_version?, method}, extensions?
}
```

- The authoritative evidence is the immutable source bytes named by `File.sha256`
  plus `SourceRef`. On read, verify the digest against stored bytes or the live
  file; if unavailable or mismatched, mark the result stale and use the existing
  source/lexical fallback. Enforce `0 <= start_byte < end_byte <= byte_length`,
  UTF-8 boundary validity, correct line/column mapping, and exact byte-for-byte
  round trips. A file unit covers `[0, byte_length)`; an empty file has an
  explicit empty-file status instead of a zero-length answerable unit. Display
  names, outlines, aliases, snippets, embeddings and summaries never become
  cited source. For non-UTF-8 bytes, use an opaque byte-backed file unit if
  supported or report failed extraction; do not silently change the digest by
  lossy decoding.
- `repository_id` and `root_id` namespace relative paths and avoid collisions
  across roots. `revision` can name a commit, but the actual snapshot is
  `sha256(schema_version, root selection, frontend versions, sorted (root_id,
  relative_path, file sha256))`. A dirty checkout is a valid content-addressed
  snapshot. File IDs are namespaced by root and path. Unit IDs are deterministic
  over `(file_id, file sha256, kind, source byte range, ordinal for coincident
  spans)`; fact IDs add `(subject, site, kind, target spelling/target, ordinal)`.
  Unchanged files can retain IDs between snapshots, but **only within-snapshot
  identity is guaranteed**. A rename, edit, macro expansion, overload, anonymous
  function, or generated declaration must not be matched across revisions by
  name/range inference; a separate diff/identity layer can do that later.
- `kind` is deliberately small; `subtype` and `extensions` retain e.g. Go
  receiver, Python decorator, Rust trait/impl/macro, TypeScript overload and
  generated/original origin information. A method is not modeled as a generic
  function solely because a current frontend labels both `function`. Preserve
  distinct declarations with shared names/signatures. `parent_id` is lexical
  containment, not a claim about type inheritance. `signature` and
  `documentation` are optional until their **own** source slices are verified;
  derived strings belong in a projection if they have no exact source mapping.
- `Fact.site` is the source span *supporting the observation* (e.g. the call
  expression or import statement). `contains`/a recognized `calls` site can be
  syntactically observed. Matching a call spelling to one repository definition
  is only `name_candidate` even if codegraph currently calls it `resolved`.
  Put name-matched targets in `candidate_ids`, not `object_id`. Only a
  type-aware frontend may emit `type_resolved` with a bound `object_id`;
  syntactic containment may also bind an `object_id`. Keep `ambiguous`
  candidate sets and `unresolved` spelling rather than inventing targets. No
  uncalibrated numeric confidence is needed: status + explicit provenance
  expresses the strength and method. Do not emit a relationship whose `site`
  is unavailable.
- In v1, calls/guards/returns/reads/writes are **facts attached to the smallest
  enclosing unit**, not separate statement nodes. Add bounded statement units
  only when a query needs independent answerable evidence, with a distinct
  source span and separately tested projection. Never infer semantic equivalence
  of exceptions, macros, async, data flow, or language-specific constructs.
- A file with no supported grammar becomes an `opaque`/file source unit with
  bounded retrieval windows referencing exact byte spans. Parse errors make
  extraction `partial`: retain only demonstrably sound units outside error
  regions, add opaque coverage for the rest, and abstain from uncertain facts.
  `complete` with zero calls is different from `partial`/`opaque` with unknown
  relationship coverage. Generated source carries a source ref into the
  generated file; origin mapping is optional and must not be guessed.

### Four serialized source-map examples

These are **illustrative, independent ASCII mini-files**, not claimed outputs
from the frozen benchmark fixtures. Each `sha256` below is of the entire
literal source including its trailing `\n`; offsets are verified against the
literal UTF-8 bytes. They show the same SourceRef schema across four syntaxes,
while the much larger language-specific AST/frontends remain free to differ.

```json
[
  {
    "file": {"file_id":"demo/go/add.go","language":"go","relative_path":"go/add.go","sha256":"df26235505695db870561db55f78b595d2130f5499f38063dd8e6d5b06eafe60","byte_length":35,"literal_source":"package demo\nfunc add() { save() }\n"},
    "unit": {"id":"demo/go/add.go@13:34:function","kind":"function","name":"add","qualified_name":"demo.add","source":{"file_id":"demo/go/add.go","sha256":"df26235505695db870561db55f78b595d2130f5499f38063dd8e6d5b06eafe60","start_byte":13,"end_byte":34,"start":{"line":2,"column_byte":0},"end":{"line":2,"column_byte":21}}},
    "fact": {"kind":"calls","subject_id":"demo/go/add.go@13:34:function","target_spelling":"save","status":"unresolved","site":{"file_id":"demo/go/add.go","sha256":"df26235505695db870561db55f78b595d2130f5499f38063dd8e6d5b06eafe60","start_byte":26,"end_byte":32,"start":{"line":2,"column_byte":13},"end":{"line":2,"column_byte":19}}}
  },
  {
    "file": {"file_id":"demo/python/add.py","language":"python","relative_path":"python/add.py","sha256":"61a887bb078454c091d84d21e393845236c19a063e0dad43ceaf79e1fe33ac71","byte_length":22,"literal_source":"def add():\n    save()\n"},
    "unit": {"id":"demo/python/add.py@0:21:function","kind":"function","name":"add","source":{"file_id":"demo/python/add.py","sha256":"61a887bb078454c091d84d21e393845236c19a063e0dad43ceaf79e1fe33ac71","start_byte":0,"end_byte":21,"start":{"line":1,"column_byte":0},"end":{"line":2,"column_byte":10}}},
    "fact": {"kind":"calls","subject_id":"demo/python/add.py@0:21:function","target_spelling":"save","status":"unresolved","site":{"file_id":"demo/python/add.py","sha256":"61a887bb078454c091d84d21e393845236c19a063e0dad43ceaf79e1fe33ac71","start_byte":15,"end_byte":21,"start":{"line":2,"column_byte":4},"end":{"line":2,"column_byte":10}}}
  },
  {
    "file": {"file_id":"demo/rust/add.rs","language":"rust","relative_path":"rust/add.rs","sha256":"19d25913e7bca05189ed065fe5cd9998ccd73ccee28f6862b9ed06d0ecb2c010","byte_length":21,"literal_source":"fn add() { save(); }\n"},
    "unit": {"id":"demo/rust/add.rs@0:20:function","kind":"function","name":"add","source":{"file_id":"demo/rust/add.rs","sha256":"19d25913e7bca05189ed065fe5cd9998ccd73ccee28f6862b9ed06d0ecb2c010","start_byte":0,"end_byte":20,"start":{"line":1,"column_byte":0},"end":{"line":1,"column_byte":20}}},
    "fact": {"kind":"calls","subject_id":"demo/rust/add.rs@0:20:function","target_spelling":"save","status":"unresolved","site":{"file_id":"demo/rust/add.rs","sha256":"19d25913e7bca05189ed065fe5cd9998ccd73ccee28f6862b9ed06d0ecb2c010","start_byte":11,"end_byte":17,"start":{"line":1,"column_byte":11},"end":{"line":1,"column_byte":17}}}
  },
  {
    "file": {"file_id":"demo/ts/add.ts","language":"typescript","relative_path":"ts/add.ts","sha256":"2157b9ac8c5c5dd8b562e5e97f831ce85248502a559b1d105374da0c47968c6e","byte_length":27,"literal_source":"function add() { save(); }\n"},
    "unit": {"id":"demo/ts/add.ts@0:26:function","kind":"function","name":"add","source":{"file_id":"demo/ts/add.ts","sha256":"2157b9ac8c5c5dd8b562e5e97f831ce85248502a559b1d105374da0c47968c6e","start_byte":0,"end_byte":26,"start":{"line":1,"column_byte":0},"end":{"line":1,"column_byte":26}}},
    "fact": {"kind":"calls","subject_id":"demo/ts/add.ts@0:26:function","target_spelling":"save","status":"unresolved","site":{"file_id":"demo/ts/add.ts","sha256":"2157b9ac8c5c5dd8b562e5e97f831ce85248502a559b1d105374da0c47968c6e","start_byte":17,"end_byte":23,"start":{"line":1,"column_byte":17},"end":{"line":1,"column_byte":23}}}
  }
]
```

Production serialization additionally includes `schema`, `schema_version`,
snapshot/root/frontend identity, `Unit.origin`, `Fact.id` and
`Fact.provenance`; the compact examples only illustrate source mapping. A
conformance fixture must validate complete production records, not these
abbreviations.

## Frontend mapping and boundaries

| Input | IR v1 mapping | Required adapter work / abstention |
| --- | --- | --- |
| TypeScript code extractor and its Go/Python/Rust adapters | Existing symbol name, type, scope, signature and docs → candidate unit attributes; original parsed node and range → source ref; one symbol → one IR unit before window splitting. | Map `web-tree-sitter` coordinates to source bytes; verify signature/docs slices; distinguish functions/methods and declarations with coincident names; record syntax error coverage. For embedded Vue/Svelte scripts, map from the **original file's** bytes or use opaque fallback. |
| Rust `zg-engine` extractor | AST declaration + byte-based entity source → unit and source ref; `CodeMetadata` supplies optional attributes. | Use parser ranges, not content-derived retrieval IDs; add explicit frontend/extraction status and lexical parent; keep relative fragment offsets in the projection only. |
| `zg-codegraph` sidecar | `files[].sha256` and nodes/ranges → candidate files/units; `defines` → containment; observed call site → `calls` fact; target name → spelling. | Join only if repository/root/path **and file digest** match the IR snapshot; downgrade name-unique `resolved` to `name_candidate`, retain ambiguity; do not import package nodes as answerable source units or imports without source ranges. Handle duplicate-name graph node collapse as coverage loss, not silent IR deduplication. |
| Optional SCIP import | `Document` and definition `Occurrence` → possible unit binding; reference `Occurrence` → possible `references` fact; `SymbolInformation` and `Relationship` → symbol kind and candidate implementation/type relationships. | Validate index scope, complete source snapshot, positions, producer precision and exact source spans first. Keep SCIP symbols as external locators, not IR IDs; only emit anchored facts with resolution status warranted by that producer. |
| All frontends | Syntax details without a common equivalent → `extensions`; unknown/unsupported regions → opaque source. | No type/flow-resolved `reads`, `writes`, `guards`, `implements`, or `tests` in v1 until a language adapter supplies and validates actual anchored evidence. |

## SCIP interoperability (optional enrichment after IR conformance)

Engram's [SCIP spike direction](https://github.com/jordigilh/engram/blob/40d4f58/docs/SEMANTIC_CODE_INTELLIGENCE.md#L179-L202)
assigns SCIP reproducible semantic snapshots while zvec-grep handles active
worktree retrieval and Serena supplies live, type-aware navigation. SCIP is a
language-neutral *exchange format for indexer output*, not a byte-for-byte code
body or lexical/vector search format. There are [indexers for all four target
languages](https://scip-code.org/), though their precision, coverage,
dependency requirements and freshness must be measured independently. Pilot
TypeScript, Python and Rust first as in the Engram spike; include Go once its
producer and snapshot behavior pass the same gates. Baseline four-language
syntax extraction does not depend on installing any SCIP producer.

**Import contract:**

1. Bind one SCIP `Index` to an explicit IR repository/root, selected source set,
   producer name/version/arguments, index generation time and external revision
   or worktree provenance. `Metadata.project_root` and `Document.relative_path`
   establish paths, but SCIP has no native file hash or commit identity.
   Independently hash the *same bytes the indexer saw* and require their
   file-set snapshot key to equal the active IR snapshot before importing
   cross-file bindings. An index built on a clean commit cannot silently add
   bindings to a dirty worktree; matching one file is insufficient if a changed
   dependency elsewhere could affect its resolution. If whole-snapshot
   attestation is unavailable, retain the artifact as an offline comparison
   only, or abstain from semantic joins. Multiple IR roots need independently
   scoped SCIP indexes.
2. Convert every half-open occurrence range from the document's declared
   `PositionEncoding` (UTF-8, UTF-16 or UTF-32 code units, 0-based lines) to
   the IR's original-file UTF-8 byte offsets, 1-based lines and 0-based byte
   columns. Prefer SCIP's typed range when present, with its older packed range
   as fallback. Reject unspecified encoding, invalid boundaries or hash/slice
   mismatches. A definition occurrence usually spans the identifier; its
   optional enclosing range *may* map to a whole declaration, including
   decorators/docs, but verify that span against an existing IR unit before
   treating it as an answerable body. Do not manufacture a full unit from an
   identifier token alone.
3. Join a SCIP definition to an existing IR unit by scoped file, verified
   declaration span and kind; retain the SCIP `symbol` as a producer-specific
   external key in provenance, never as a cross-revision IR ID. A reference
   occurrence with a verified site may create `references` from the smallest
   containing unit to a local matched definition. External-package symbols
   remain external locators without invented local `object_id` or source unit.
   A SCIP symbol relationship such as `is_implementation` may become an
   `implements` fact only if both endpoints and a supporting *definition-site*
   range are validated; otherwise keep it as unranked enrichment/diagnostics.
   `SymbolInformation.documentation` and signature documentation are rendered
   metadata, not source evidence unless separately mapped to original bytes.
4. Carry producer identity, version, index/snapshot digest, SCIP symbol,
   occurrence role and method in `Fact.provenance`/language extensions. The
   protocol allows heuristic producers: a SCIP match is `type_resolved` only
   when the specific producer is known to use semantic resolution for that
   relation *and* the snapshot/targets match. Other matches remain
   `name_candidate`, `ambiguous` or `unresolved`. `ReadAccess`/`WriteAccess`
   roles may describe anchored occurrences if verified, but are not general
   data-flow facts. A reference to a function is not automatically a `calls`
   edge: require an independently verified call site and binding before
   attaching a call target.

**Evaluation gate:** For each pilot language, compare syntax-only IR with
SCIP-enriched IR on independently annotated definitions, references and
implementations: exact-span round trips, coverage, false bindings, unresolved
and ambiguous counts, generation cost, dependency/setup failures, and
commit/branch/dirty-worktree freshness. Compare exact navigation with
Serena/gopls/pyright/rust-analyzer/TypeScript language-service results where
available. Run any SCIP-backed search or graph ranking as a *separate* same-
engine, same-language ablation using the unchanged qrels and per-query gates.
Failure or absence of an indexer leaves the syntax-backed IR and existing
lexical/source fallback usable; it never silently changes the resolution label
or enables graph ranking.

Measure coverage against the four *independent* source-authored unit manifests
([protocol: 38 Go / 33 Python / 39 Rust / 33 TypeScript](https://github.com/jordigilh/engram/blob/40d4f58/benchmarks/semantic_search/MULTILANGUAGE_PROTOCOL.md#L10-L24)), plus an AST-annotated relation inventory per language: `(manifest units matched by exact path+source span+kind) / (eligible manifest units)`; `(verified round-trips)/(emitted units and facts)`; `(observed, name-candidate, ambiguous, unresolved, type-resolved call sites)/(manually labeled call sites)`. Break down by declaration kind, parser/frontend, errors, generated source and unsupported syntax. The Rust manifest includes six grade-0 module-declaration units: report whether these become file/module/opaque units instead of quietly dropping them. **Coverage percentages are pending measurement**, not inferred from a passing sidecar test.

## Projections, publication and rollout

1. **Extraction-only spike:** Define machine-validated IR v1 and adapters for
   Go, Python, Rust, TypeScript. Write immutable file snapshots and conformance
   fixtures; compare the old and IR path's declaration coverage and exact source
   bytes. Existing retrieval results remain the default. Test Unicode, CRLF,
   comments/decorators, duplicate names/overloads, closures, macros, parse
   errors, unsupported files, generated files, moves and embedded scripts.
2. **Optional, non-blocking SCIP interoperability spike:** Once the syntax-backed
   IR passes conformance, import one commit/snapshot-attested SCIP artifact per
   pilot language in shadow mode; measure source-range conversion and semantic
   binding fidelity before emitting canonical facts. This follow-up does not
   hold up syntax-only projections. Keep the importer and producer dependencies
   optional. Missing or stale SCIP never blocks IR.
3. **Same-snapshot projections:** From *only the validated IR snapshot* derive
   `lexical_text` (start by reproducing the current combined field), vector
   embedding inputs, bounded retrieval windows/parent grouping, and optionally
   graph adjacency. Each derived record stores `ir_snapshot_id`, projection
   schema/version and model identity where applicable. A lookup joins by IR
   unit ID + source ref, then returns source bytes as evidence. Keep raw exact/
   regex search and existing source/lexical-only fallback available.
4. **Publish consistently:** Build candidate IR + projections in a staged
   generation from the *same read of each selected source file*; validate all
   digests and source refs, then publish one active snapshot manifest last.
   Readers pin the manifest for the whole request and never join records across
   different snapshot IDs. Rust's full-rebuild generation switch is a useful
   starting point, but incremental writes and the sidecar need this additional
   join/publication rule. If the first spike cannot stage both stores, leave IR
   shadow-only and fall back whenever hashes/IDs disagree.
5. **Incremental rules:** Reparse changed/added files, tombstone deleted/moved
   paths, invalidate their units/facts/projections, and re-resolve dependent
   *candidate* edges before publishing the new manifest. Compare incremental
   output byte-for-byte with a full build on the same snapshot. Schema, byte-map
   convention or incompatible frontend/grammar change → full IR rebuild;
   lexical projection change → reproject IR (and rebuild its FTS index);
   embedding input/model change → re-embed affected units; resolver change →
   re-resolve edges. None of these can mix two generations. Cross-revision
   symbol matching is a later, explicit feature.
6. **Evaluation gates:** Preserve each same-engine lexical-only baseline.
   First require 100% exact byte round-trips for *emitted* source-backed
   records, no silently dropped unsupported regions, and per-language
   extraction/relationship coverage reported against adjudicated inventories;
   compare declaration coverage with the existing extractor rather than
   assuming it is complete. Then compare IR-backed projections to
   the same-engine baseline on each lane's complete qrels, all eight per-query
   deltas, raw and normalized top-10 ranks, and all four @10 metrics. Require
   at least one aggregate improvement and no decline in the other metrics **per
   language/engine** before that lane can be enabled; diagnose individual query
   losses. Run adjudicated real-repository queries before generalization.
   Only after projection correctness, evaluate Proposals 1–4 separately (no
   implicit graph expansion or reranking in the IR spike). Issue [#7](https://github.com/jordigilh/zvec-grep/issues/7)
   separately evaluates citations, answer sufficiency, tokens and follow-up
   reads from an optional context renderer consuming IR references. A SCIP
   enrichment must pass the same source, freshness and per-language gates before
   it can influence any of these experiments.

**Review decision sought:** agree on the v1 file/unit/fact/source-ref contract,
the conservative name-candidate provenance rule, per-language conformance and
coverage measurement, the one-snapshot publication rule, and an optional SCIP
importer that can add verified semantic facts without becoming the source or
freshness authority. Empirical ranking benefit and default enablement remain
decisions for the paired gates.
