# Rust native SCIP reassessment: snapshot producer versus LSP adapter

**Decision (2026-09-25):** Rust is a **distinct viable native SCIP snapshot lane**. The [official SCIP Rust indexer](https://github.com/scip-code/scip-rust#readme) is a thin wrapper around `rust-analyzer scip .`, so the existing local rust-analyzer binary already supplies the official Rust production path; no separate `scip-rust` installation is needed for this experiment. On frozen Rust source, its index has explicit per-document UTF-8 range encoding and independently verifiable token and definition-declaration spans. For **batch/immutable Rust semantic snapshots**, evaluate a snapshot-attested native SCIP importer directly rather than assuming Rust must be reconstructed through an LSP `documentSymbol`/`references` crawl. Keep the LSP path available for live interactive navigation. Neither producer is yet approved as the default IR binding source: the measured strict join to current syntax IR is partial, and this SCIP artifact contains **no implementation relationships**.

## Same-source run and format evidence

On the existing isolated source copy `/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/lsp-ir-v13-recheck-20260925/rust`, run `rust-analyzer scip . --output index.scip` (version `0.0.0 (f938641be5 2026-08-10)`). This fresh run took **3.14 s wall**, peak RSS **636,665,856 bytes** on this macOS arm64 host; the 70,287-byte `index.scip` SHA-256 is `840d57ff93f918317641f6c9c782f1dffec07b5b2fe24bac9c329ee8a85dbaf4`. Source-set digest (manifest-selected 14 files) is `a9f5020761eb5c66291715068224f152c120545069c090f5746cc086d55fefc3`. Its `Metadata.project_root` resolves to that same staged root. The official `scip.proto` used to decode has SHA-256 `b38021b65ef90cbbf6af9c829ff75192859ad9b5da05439ef154bea4ceb2bf03`.

| Property | Measured Rust SCIP output |
| --- | ---: |
| Selected documents | **14/14**, no extras |
| `Document.position_encoding` | **14/14 UTF-8 byte offsets**; no unspecified documents |
| Legacy versus typed primary ranges | 573 legacy, 0 typed |
| Primary occurrence byte slices | **573/573** valid, nonempty over original UTF-8 |
| Definition occurrences | 131 |
| Definition enclosing declaration range | **131/131** present and containing the definition token |
| Reference occurrences | 442 |
| Reference enclosing expression range | 4/154 valid; **150/154** present but do not enclose the reference token |
| References with a unique definition in selected files | **206/442**; remaining 236 have no *selected local* definition (often external/sysroot; not all unresolved) |
| `SymbolInformation.relationships` / implementations | **0 / 0** |

**Correction to the earlier blanket enclosing-range concern:** all 150 failed containment checks concern **reference** enclosing ranges, not definition enclosing ranges. A strict importer can source-map primary reference tokens and may validate definition body ranges; it must *ignore/reject* the unreliable optional enclosing **reference expression** ranges. A reference token alone is not proof of a call-expression span.

An additional **real producer** control created only in a disposable temporary Cargo project placed `é🚀` before `target` and used CRLF: rust-analyzer emitted two UTF-8-encoded documents, with `target.rs` declaration `[42,48)` and `lib.rs` cross-file use `[57,63)`. Both UTF-8 byte slices equal `target` and share a SCIP symbol. Control source-set digest `3179f00ecb571798c607fc8bd3bdfd3a23b7edc6f30f7b1833253cb3defbecaf`; SCIP digest `64ad728d7893611f6c4bef9cb143f203317645b1a24ee4cc1c68b7f873d59ce8`. This supplements the earlier synthetic converter test with actual Rust producer output.

## Comparison with the same v1.3 Code IR/LSP snapshot

[`scripts/rust_scip_reassessment.py`](../scripts/rust_scip_reassessment.py) rejects schema drift, cross-root metadata, unknown document encoding, missing/extra files and stale whole-file-set/IR hashes. It scopes `local N` symbol IDs to their document, decodes real primary and optional ranges and compares the *same source sites* with the separately built rust-analyzer LSP artifact. [`scripts/rust_scip_shadow_import.py`](../scripts/rust_scip_shadow_import.py) normalizes only 123 uniquely scoped, typed Rust definition symbols with valid enclosing ranges and 197 corresponding local reference sites; it joins to current `web-tree-sitter-ir-v1.3` syntax units only on unique file/name/kind/exact full declaration span and validated token containment. The resulting **34/197** strict reference facts (versus 49/104 in the separately bounded LSP document-symbol pass) form a schema-valid *experimental* `scip-shadow-ir-rust.json`, SHA-256 `9987bebb01f9c11c263819524bb89a8b0b0d8700e0b742ec3d785f5fbd02c641`. They retain the syntax IR snapshot ID and passed its actual `validateSnapshot`; this is not a fair coverage ranking because the source-site denominators differ.

| Cross-producer comparison | Measurement / interpretation |
| --- | --- |
| LSP-derived reference sites present as some SCIP occurrence | **104/104** |
| LSP sites with a unique selected-local SCIP definition | **100/104** |
| SCIP selected-local references whose site appears in bounded LSP artifact | 101/206 occurrences |
| Of those shared occurrences, identical definition-token target | **89/101** |
| Different definition-token target | **12/101**: 11 module targets (SCIP points to module file body; LSP often points to the `mod` declaration in its parent), one `allowed_ids` struct-field shorthand (SCIP parameter versus LSP field); do not adjudicate these as false bindings by symbol string alone. |
| Independently source-authored navigation labels | **5/5** correct SCIP definition-site targets |
| Labels strictly bound to current syntax IR using definition enclosing span/kind | **4/5** (same `#[derive]`/canonical unit-span mismatch that blocks the LSP strict join) |

The artifact has 131 raw definition occurrences, but only 33 have a unique source token/name/kind match to existing syntax units; 30 of those also have an exact full enclosing declaration span match. Only 34 reference occurrences target those exact-span units. The main blocker is **canonical syntax IR unit alignment/coverage**, not SCIP range encoding. The 236 references without a selected-local definition need external-package locators or a broader attested dependency corpus. Do not invent a local `object_id` for them; do not infer `implements`, calls or type flow from a reference symbol. SCIP metadata lacks source hashes and revision; the same whole-selected-file-set and config attestation remains mandatory before publishing any cross-file fact. The `scip-shadow-ir-rust.json` facts are shadow-only candidate `type_resolved` schema shapes used to exercise the validator, not a production precision claim.

**Routing recommendation:** For Rust *frozen, complete source snapshots*, use `rust-analyzer scip` as a candidate offline semantic exchange/index input and compare it with syntax-backed IR. Do not build a redundant Rust LSP reference crawl merely to solve SCIP position encoding—Rust SCIP already declares it. Reserve rust-analyzer LSP for live workspace navigation/implementation requests or targeted disagreement adjudication. Promotion still requires strict declaration join improvements (e.g. correct Rust attribute/decorator canonical spans), an independently labeled relation inventory beyond five examples, explicit module/shorthand resolution policy, full source/dependency attestation and measured `implements` support from the chosen producer. No default routing, ranking or active implementation worktree changed.

## Reproduce

Start with a **new** isolated ROOT staged using `python3 scripts/scip_spike.py stage "$ROOT"`; install no `scip-rust` wrapper. `scripts/export_syntax_ir.mjs` reads the sibling Code IR implementation **only** to extract the current syntax snapshot, and must record/verify its compiled implementation digest. LSP is needed below **only for the optional comparison**, not for the SCIP importer:

```sh
python3 scripts/scip_spike.py schema "$ROOT"
( cd "$ROOT/rust" && rust-analyzer scip . --output index.scip )
node scripts/export_syntax_ir.mjs "$ROOT" MODULE FIXTURES rust
python3 scripts/rust_scip_reassessment.py "$ROOT" --unicode-control
# For same-site LSP comparison only: first run scripts/lsp_code_ir_spike.py "$ROOT" --full rust
python3 scripts/rust_scip_reassessment.py "$ROOT"
python3 scripts/rust_scip_shadow_import.py "$ROOT"
IR_ARTIFACT_PREFIX=scip-shadow-ir node scripts/validate_shadow_ir.mjs "$ROOT" MODULE rust
python3 scripts/lsp_ir_shadow_join.py "$ROOT" --check-labels --scip rust
```

`MODULE` is the read-only sibling path to `dist/engine/code-ir/index.js`; `FIXTURES` is the Engram fixture directory. For reproduction with the pinned **v1.3** measurement, verify the exporter records compiled `dist/engine/extraction/code/ir.js` SHA-256 `273959e59de67e60f23ad4aed192edf430679afb60eb4dd5eb0abd5f0c72c856` before comparing counts. On a later frontend version, report a new lane rather than mixing numbers.
