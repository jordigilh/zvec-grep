# Code IR v2 frozen retrieval gate (issue #8)

**Review slice:** child of [PR #15](https://github.com/jordigilh/zvec-grep/pull/15),
which is stacked on [PR #14](https://github.com/jordigilh/zvec-grep/pull/14).
Neither this adapter nor v2 changes default search ranking. SCIP is not an input.

## Plan and test contract

1. Write failing tests for strict source/QREL/provenance validation, unmapped or
   unjudged results, exact eight-query alignment, gate computation and per-query
   loss reporting. Use the *unchanged* Engram scorer and source-unit span mapper;
   do not alter frozen sources, manifests, truth or qrels. A synthetic negative
   test must reject mismatched raw query IDs before scoring.
2. Reuse the same TypeScript engine for the control and candidate, the same
   `local/potion-code-16m-v2` CPU model and same direct-hybrid query path,
   fixed top 10. Build fresh baseline and candidate source roots and indexes
   per language; the candidate consumes the actual published/read-validated
   `source-metadata-entity-v1` v2 projection, never reimplements its policy.
   Record files, model and runtime hashes and raw ranks. The baseline uses the
   current syntax extractor, not the v1 projection. Leave unsupported file
   fallback for a separate evaluation rather than silently mixing indexes.
3. Run each frozen Go/Python/Rust/TypeScript lane independently. Normalize raw
   ranked byte-backed source ranges to immutable qrel unit IDs, then score with
   Engram's adjudicated-qrel scorer. Record per-query deltas (including losses)
   and overall nDCG/MRR/recall/precision@10, source and model provenance. A
   lane passes only if **one** overall metric improves and **none** regresses;
   query-level losses still need review. Do not promote any lane from this
   synthetic set alone; real-repository independent judgments are separate.
4. Document reproduction and the exact blocker if model, zvec or fixture
   inputs are unavailable. Keep all generated outputs in fresh temporary
   directories outside this checkout and Engram. If a lane fails, leave it
   disabled. No default ranking/configuration change belongs in this PR.
