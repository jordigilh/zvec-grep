# Go Code IR / SCIP qeval (2026-09-25)

## Result

The frozen Go qeval does **not** pass for either current candidate projection.
The ordinary syntax/extractor arm reproduces the archived zvec control, while
the sidecar's current Code IR projection loses retrieval quality. Adding
source-attested SCIP reference-target names to that projection loses more.
Keep both experimental arms out of default search.

| Go arm | nDCG@10 | MRR@10 | Recall@10 | Precision@10 | Gate vs syntax-only |
|---|---:|---:|---:|---:|---|
| Syntax-only control | 0.588774 | 0.733333 | 0.677083 | 0.250000 | Baseline |
| Code IR projection | 0.489163 | 0.718750 | 0.543750 | 0.200000 | **Fail** |
| Code IR + SCIP references | 0.432410 | 0.675000 | 0.502083 | 0.187500 | **Fail** |

The Code IR arm changes all four aggregate metrics negatively versus control:
**−0.099611 nDCG, −0.014583 MRR, −0.133333 recall, and −0.050000 precision**.
The SCIP-reference arm is lower again. The eight per-query results are preserved
in `metrics-k10.json` and `comparison.json` in the run artifact directory.

| Query | Control nDCG / recall | Code IR nDCG / recall | Code IR + SCIP nDCG / recall |
|---|---:|---:|---:|
| `record-discovered-membership` | 0.522 / 0.500 | 0.535 / 0.750 | 0.535 / 0.750 |
| `reject-undiscovered-workflow` | 0.191 / 0.250 | 0.000 / 0.000 | 0.000 / 0.000 |
| `forward-labels-to-discovery` | 0.997 / 1.000 | 0.807 / 1.000 | 0.715 / 1.000 |
| `preserve-membership-on-retry` | 0.227 / 0.400 | 0.196 / 0.200 | 0.176 / 0.200 |
| `interactive-selection-guard` | 0.936 / 1.000 | 0.881 / 1.000 | 0.850 / 1.000 |
| `validate-workflow-parameters` | 0.627 / 1.000 | 0.547 / 0.667 | 0.250 / 0.333 |
| `empty-discovery-fails-closed` | 0.436 / 0.600 | 0.271 / 0.400 | 0.257 / 0.400 |
| `copy-selected-workflow-details` | 0.775 / 0.667 | 0.678 / 0.333 | 0.676 / 0.333 |

## Evaluation contract and scope

- Source and qrels are Engram's frozen `go-workflow-discovery-v1`: **10 Go
  files, 7,872 bytes, 38 answer units, 8 queries**. Source-set SHA-256 is
  `94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f`.
- The source, manifest, truth and complete qrels were revalidated before each
  run; none were edited. The evaluator uses the existing source-unit `@10`
  normalization and complete adjudicated qrels.
- Every arm used fresh source roots, the same Code IR worktree/search runtime at
  `139fe4e7eda0c084e85eb641003a2e9204a87c60`, zvec 0.7.0, CPU, and
  `local/potion-code-16m-v2`. Syntax-only reproduces the saved lexical control's
  metrics, which is a useful check that the new harness is on the same search
  path.
- The Code IR projection arm indexes the sidecar's source windows using the
  same zvec FTS schema and `searchWorkspaceIndex` fusion logic. It excludes
  whole-file/opaque records because they are not answerable manifest units.
  This projection-to-index adapter exists only in the qeval script; the
  application sidecar is still not connected to search.
- The SCIP arm adds target qualified names from **117 strictly joined,
  same-snapshot Go references** to both FTS and vector input. This is an
  explicit reference-text ablation, not graph expansion, a call inference, or a
  ranking feature in the product.

## Interpretation

The IR-source gate and retrieval gate give different answers. A fresh v1.5 Go
snapshot independently matched all **38/38** manifest declaration spans and
kinds against Go's parser AST, and all source token references validated. The
Code IR snapshot contains 68 units total, 58 answerable non-file units. This
confirms the exact source coverage improvement; it does **not** establish that
the current projection text is a useful replacement for the existing
`CodeExtractor` representation.

The largest regression is the policy query `reject-undiscovered-workflow`:
the existing extractor returns `Validator.IsAllowed` in the top ten, while the
current IR projection returns no qrel-relevant unit there. The current sidecar
projection emits names, qualified names, name parts and scope, but not all
signature/documentation metadata present in the existing retrieval projection.
That difference is a concrete next ablation to test, not yet a proven causal
explanation. Adding SCIP reference-target names to the current projection does
not repair the loss and lowers validation-query quality further.

## Provenance and reproduction

The pinned run manifest records the fixture/qrels hashes, frontend, model cache,
runtime and producer artifacts. Key values:

- Code IR snapshot: `38924ec8adb36ec929b2a9ac01f3ec09ce786fe8d355fad98e17a42507dc6be6`
  (`web-tree-sitter-ir-v1.5`)
- Go AST inventory: SHA-256 `42b7fef4f79bc84ec891666caea94696f3cf40e41d522d60eaf0d7268c977faf`
- Custom scip-go binary: `eb09bc8ca1315c474308b9f6ca356f4ec744b45f9986519d65319c825b214e0e`
- SCIP index: `994e177b83353f9f5c3a1b7bc45a9f7a8171aeb66ddde463acb3575222b55e82`
- Strict SCIP shadow: `7697906e0355c193e78c111bc75afe453926ece6cd53a74b0099c4f8fde0d30a`
- Raw evidence: `a5480230097806fdeac34fe9104eb75bbb77cd0d22cec1201359b914a6d13785`
- Normalized ranks: `59dfcb511bb7cd1ff1dd575082acf2e3443fabc0624cef3523d4f8f357ec55d7`
- Metrics: `7632bdb7bd70d24fa55f5c21d3c0dc801347118434d0ff2f86586b67bcf8e776`

Run artifacts are under `/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/code-ir-scip-qeval-matrix-go-20260925/results/` on the evaluation host. Reproduce with a new empty work/output pair:

```sh
python3 scripts/replay_ir_projection_qeval.py \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --runtime-root /path/to/zvec-grep-code-ir-design \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /tmp/go-ir-qeval-NEW \
  --output-dir /tmp/go-ir-qeval-NEW/results \
  --scip-shadow /path/to/staged-root/shadow-ir-go-v15.json \
  --scip-index /path/to/staged-root/go/index.scip \
  --scip-binary /path/to/custom/scip-go-spike \
  --scip-producer "scip-go@0.2.7 custom PR #298" \
  --expected-scip-facts 117
```

The same runner was used for the full four-lane Code IR and SCIP-reference-text
matrix, summarized in [`code-ir-multilanguage-qeval-results.md`](./code-ir-multilanguage-qeval-results.md).
The producer-attributed Go rerun in that matrix has raw-evidence SHA-256
`6de08e062c7f2000c0a1c5691ce623cbbd9f0cecfb2a3fb70366e7ed11ad477b` (same
normalized-rank and metric hashes above). No result here changes product
defaults or enables a SCIP-backed search path.
