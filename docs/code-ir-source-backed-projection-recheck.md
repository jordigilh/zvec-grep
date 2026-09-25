# Compiler-style Code IR projection: source-backed parity recheck (2026-09-25)

## Decision

The compiler-style split is feasible on the measured Go fixture: Code IR v1.5
matches the **38/38 independently inventoried Go declarations** by byte span and
kind, while the patched scip-go producer strict-joins **117/117 selected-local
semantic references**. Neither fact alone implies that a retrieval projection
should rank better. Source-attested signatures/comments and a separate policy
for which IR units become search hits nearly reproduce the old Go retrieval
score, but still **do not pass** its no-regression gate. Flatly adding SCIP
target names lowers quality again. Keep the semantic facts as validated
relationships, distinct from answerable search text.

## A/B setup

[`scripts/source_backed_ir_metadata.mjs`](../scripts/source_backed_ir_metadata.mjs)
reuses the existing extractor *only in this experiment* to obtain signature
and adjacent-comment candidates. It accepts one unambiguous matching Code IR
unit only after checking path, name/scope, original UTF-8 bytes, declaration
span, file SHA-256, and signature/comment source slices. Non-adjacent comments
abstain. An unchanged Code IR snapshot and source windows remain the answerable
source. A production frontend should derive these refs on the same parse/read,
rather than run a second parser.

[`scripts/ir_retrieval_unit_policy.mjs`](../scripts/ir_retrieval_unit_policy.mjs)
keeps **20 Go struct-field** and **12 Python class-attribute** units in the IR
but omits them as standalone ranked search results, matching the older
extractor's entity granularity. Their semantic `object_id` bindings remain
available. Rust and TypeScript units are unchanged by this policy on these
fixtures. No qrels, fixture sources, ranking weights or Code IR design-worktree
files were edited.

Every fresh run used the same TypeScript search engine at committed
`139fe4e7eda0c084e85eb641003a2e9204a87c60`, zvec 0.7.0,
`local/potion-code-16m-v2` on CPU, the eight frozen queries for its language,
and Engram's complete source-unit qrels/scorer. Only opt-in index text, vector
input and the projection's standalone-entity selection change between arms.
The whole-unit/opaque spans are excluded from all IR search arms.

## Results

Four independent language lanes; entries are **nDCG@10 / MRR@10 / Recall@10 /
Precision@10** (not a cross-language average):

| Language | Syntax-only control | Current IR projection | Source-attested signature+docs, all units | Legacy entity granularity + source metadata | Gate on last arm |
|---|---|---|---|---|---|
| Go | 0.588774 / 0.733333 / 0.677083 / 0.250000 | 0.489163 / 0.718750 / 0.543750 / 0.200000 | 0.562941 / 0.739583 / 0.641667 / 0.237500 | **0.586157 / 0.733333 / 0.677083 / 0.250000** | **Fail** (nDCG −0.002617) |
| Python | 0.546521 / 0.547917 / 0.689583 / 0.250000 | 0.492982 / 0.531250 / 0.622917 / 0.225000 | 0.530904 / 0.524107 / 0.647917 / 0.237500 | **0.537513 / 0.530357 / 0.689583 / 0.250000** | **Fail** (nDCG −0.009007, MRR −0.017560) |
| Rust | 0.416330 / 0.455556 / 0.560417 / 0.200000 | 0.441125 / 0.465774 / 0.622917 / 0.225000 | 0.433442 / 0.455853 / 0.585417 / 0.212500 | same as signature+docs (no units excluded) | Pass aggregate; query losses remain |
| TypeScript | 0.426791 / 0.591667 / 0.550000 / 0.200000 | 0.460006 / 0.600000 / 0.616667 / 0.225000 | 0.464443 / 0.591667 / 0.633333 / 0.225000 | same as signature+docs (no units excluded) | Pass aggregate; query losses remain |

The Go query `reject-undiscovered-workflow` regains `Validator.IsAllowed`
within the top ten (rank 4 in the all-unit signature+doc run, vs no relevant
hit in the original IR projection). In the entity-parity+metadata arm it
matches the baseline's query-level nDCG/recall. Remaining Go losses include
one useful interactive-guard unit falling out of @10; gains elsewhere offset
it in aggregate recall. Python's biggest remaining ordering loss is
`preserve-membership-on-retry`, not missing declarations. These are diagnostic
leads from frozen qrels, **not** templates for query-specific boosts.

Route isolation on the entity-parity projection did not resolve the remaining
losses: Go FTS-only metadata scored 0.563293 nDCG / 0.637500 MRR; Go
vector-only scored 0.536054 / 0.775000. Python FTS-only scored 0.535891 /
0.551190; Python vector-only scored 0.494514 / 0.512500. Neither route-only
arm passes all four aggregate metrics. Rust's vector-only arm **does regress
MRR**, despite the combined arm's aggregate pass, which reinforces keeping
the route ablations separate.

The strongest Go entity-parity+metadata arm still regresses after flat SCIP
reference-name injection: **0.545864 nDCG / 0.729167 MRR / 0.610417 recall /
0.225000 precision**. It used the same 117 strictly joined Go facts as the
earlier ablation, with no change to source bytes. SCIP binding correctness and
retrieval rank usefulness are distinct gates.

## Source and provenance limits

The Go metadata extraction found 38 unambiguous name/span matches, 38
source-attested signatures and four adjacent source-backed comments. Python
found 33 signatures and no such comments; Rust found 36 signatures and no
comments; TypeScript found 33 signatures and no comments. The Go Code IR
snapshot still has 68 units (including fields); the filtered projection ranks
38 standalone units. The Python snapshot still has 53 units and ranks 33 in
the filtered projection. Unmatched or unsupported metadata abstains and does
not change IR identities.

These synthetic eight-query source-unit qevals are regression evidence, not
whole-repository semantic precision or agent-answer quality. Only the Go
fixture has a separately adjudicated exact byte-span/kind inventory covering
all of its manifest declarations. The Rust/TypeScript aggregate passes do not
authorize promotion while their query-level regressions and independent
relation inventories remain unresolved. An immutable Kubernaut source-map
probe was successful; a **separate, independently judged real-Kubernaut
retrieval corpus** is still required before a real-repo quality claim.

## Artifacts and reproduction

Each result directory contains `raw-runs.json`, `normalized-runs.json`,
`metrics-k10.json`, `comparison.json`, and `run-manifest.json` with fixture,
model, frontend, SCIP (if present), qrels, and output digests. The final roots
on this host are under
`/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`:

| Language | Result directory | Metrics JSON SHA-256 |
|---|---|---|
| Go (including custom SCIP arm) | `code-ir-final-source-parity-go-results-20260925/` | `5531b1f56bfdd4e3e5c773abf1e2b9714916e7d00eea8cd4d5f96e418be040ed` |
| Python | `code-ir-final-source-parity-python-results-20260925/` | `d2b41a8812d6d20f14f852e54669a603dc132ad226b110e7d8262fbace024360` |
| Rust | `code-ir-final-source-parity-rust-results-20260925/` | `65b0746d8c9523368c8e0b05328cde8725a78e3aa8f7461a18a828eb16626cfc` |
| TypeScript | `code-ir-final-source-parity-typescript-results-20260925/` | `bdf50640a7fe75332fbb46fcbafb987ed3b64c020bced664e746444ac9385245` |

Use a **new** temporary `--work-dir` and `--output-dir` per lane. The Go
SCIP arm additionally needs the pinned custom producer and same-snapshot
shadow artifact:

```sh
python3 scripts/replay_ir_projection_qeval.py \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --runtime-root /path/to/zvec-grep-code-ir-design \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /tmp/code-ir-parity-NEW \
  --output-dir /tmp/code-ir-parity-NEW-results \
  --source-parity --entity-parity \
  --scip-shadow /path/to/staged-root/shadow-ir-go-v15.json \
  --scip-index /path/to/staged-root/go/index.scip \
  --scip-binary /path/to/custom/scip-go-spike \
  --expected-scip-facts 117 --summary
```

For Python, Rust, or TypeScript, use its own frozen fixture path and omit the
SCIP flags when testing projection parity alone.

## Next engineering steps

1. In the owning Code IR frontend, derive source-backed signature and
   contiguous documentation refs **during the one read/parse**. Validate
   every byte slice and retain explicit abstentions. Do not make the temporary
   second parse or this script's index a production lifecycle.
2. Preserve semantic field/attribute units as join endpoints but make
   answerable retrieval-unit selection an explicit, versioned projection
   policy, tested against more than one fixture and real Kubernaut source.
3. Implement a snapshot-scoped relationship query/diagnostic layer so SCIP
   facts can be inspected after target discovery, rather than copying
   qualified target strings into every embedding/FTS field. Independently
   judge reference/type/implementation sites (including negatives, ambiguous
   and external targets) and account for toolchain/build configuration and
   dependencies in the snapshot attestation.
4. Build real Kubernaut qrels and replay paired same-engine arms on one
   immutable revision, with source citations, per-query failure breakdown,
   model/index provenance, and unchanged default retrieval until gates pass.
