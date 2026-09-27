# Go-only retrieval timebox: no ranking pass

**Decision:** Stop this candidate-generation/source-lexical-rerank attempt.
It does **not** pass the precommitted four-metric no-regression gate against
the existing Sense-enhanced syntax baseline. There is real candidate headroom,
but merely overfetching (and changing the engine's fusion cutoff) loses MRR,
while the one source-verified lexical reranker recovers MRR at the cost of
nDCG and important per-query losses. Neither arm is enabled by default; the
four-language IR evaluation remains parked at [PR #20](https://github.com/jordigilh/zvec-grep/pull/20).

This is an exploratory result on eight **previously exposed**, source-authored
synthetic Go questions. It is not evidence that better Go semantic retrieval
is impossible, nor permission to tune weights/keywords on these eight tasks.

## Same-engine comparison: Sense-enhanced syntax index, Go only

The [bounded plan](./go-retrieval-timebox-plan.md) was committed at `0bb943f`;
the diagnosis and single source-verified rerank policy were fixed at `0895434`
and `d5304a8` respectively **before evaluating the reranked arm**. Fresh
staging/index/results outside the fork and Engram checkouts used the same
source snapshot, cached CPU model, existing hybrid fusion, original Engram
source-unit line-overlap mapper, unmodified qrels/scorer and original query
text. The original ten-result control reproduced the earlier syntax metrics
*exactly* before evaluating new arms. One paired run of the committed policy;
no subsequent ranking, query, qrel or threshold edits.

| @10 metric | Syntax baseline | Same hybrid engine at limit 30 (top 10) | Source-verified rerank of the same 30 → 10 |
|---|---:|---:|---:|
| nDCG | **0.588774** | 0.604353 (+0.015579) | 0.587148 (**−0.001626**) |
| MRR | **0.733333** | 0.726389 (**−0.006944**) | 0.747024 (+0.013690) |
| Recall | **0.677083** | 0.714583 (+0.037500) | 0.683333 (+0.006250) |
| Precision | **0.250000** | 0.262500 (+0.012500) | 0.250000 (unchanged) |
| Precommitted no-regression gate | Control | **Fail** (MRR) | **Fail** (nDCG) |

The fixture contains 38 indexed Go syntax entities. Of 34 positive qrel
**source-unit** sites, 22 overlap a baseline @10 raw hit, 10 more overlap a
raw hit somewhere in the @30 pool (some rank within its *changed* first ten),
and two remain outside @30. All 34 overlap at least one indexed raw span.
These are **not** counts of scored relevant top-ten results: Engram's mapper
can map one raw hit to multiple overlapping source units, then deduplicates
and scores the *first ten normalized units*. Thus baseline precision 0.250
is consistent with the raw-span inventory. The @30 request changes fusion's
limit-dependent rank agreement cutoff; its top-ten is not a mere prefix of
the original @10. The first diagnostic artifact called the ten additional
positives `in_pool_below_10`, an imprecise label; the final run correctly
names this category `new_in_pool` and keeps both runs immutable.

**Per-query losses matter:** Overfetch loses relevance on
`preserve-membership-on-retry` (−0.090 nDCG, −0.056 MRR, −0.200 recall),
`validate-workflow-parameters` (−0.080 nDCG, −0.333 recall), and
`empty-discovery-fails-closed` (−0.051 nDCG). Reranking loses nDCG on
`reject-undiscovered-workflow` (−0.026, MRR −0.057),
`validate-workflow-parameters` (−0.080, recall −0.333), and
`empty-discovery-fails-closed` (−0.117); it also loses recall (−0.200) on
`preserve-membership-on-retry` despite a slight nDCG improvement there.
There are wins on `record-discovered-membership` and
`copy-selected-workflow-details`. Full-precision per-query deltas and raw
rank/route traces are pinned in `comparisons.json`/`raw-runs.json`.

The source-lexical arm performs only a verified substring and hash readback
of existing indexed fragments, generic identifier segmentation/stopword
removal and fixed IDF-weighted alignment of source/name/path combined with
original hybrid rank. It introduces no IR graph edge, inferred callee,
semantic model, qrel-dependent rule or extra embedding. The partial MRR gain
is not a general semantic-accuracy improvement. There is no automatic
fallback switch or UI/product path.

## Reproduce or review artifacts

On this host, the approved external temp root is
`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/opencode/`.
The first diagnosis remains at `go-retrieval-diagnostic-results-20260926/`;
the definitive one-shot run and pinned hashes are in
`go-retrieval-timebox-results-20260926/` and its corresponding fresh work
directory. Use **new** distinct external paths for a later reproduction:

```sh
npm run build
python3 scripts/go_retrieval_timebox.py \
  --fixture /path/to/engram/benchmarks/semantic_search/fixtures/go-workflow-discovery-v1 \
  --engram-root /path/to/engram \
  --model-cache "$HOME/.engram/zvec-grep/model" \
  --work-dir /path/to/new-go-work \
  --output-dir /path/to/new-go-results \
  --rerank
```

Run revision: `d5304a8bcebceb3e5beed78d24bad16e4994b019`.
Frozen source-set SHA-256:
`94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f`.
Model cache tree SHA-256, unchanged before/after:
`9eccd62fe5ce1815096be4b5dff78708f27c686c0c70305017f23984e0524252`.
Qrels SHA-256: `0d89687ffd9aac8e6688e9c0931aeb27cca55b99d04f6379ecc5b9bba541c606`.
Rerank policy file SHA-256:
`96ca1c21f877ed374df512950ff46cc4c91af0d34e1405646ed5dea7dc82eb75`.

| Output | SHA-256 |
|---|---|
| `raw-runs.json` | `a3fe95c5f8e431cf3bbf44a10cf895df5e40b25ea624caa6213c6da218e20d56` |
| `normalized-runs.json` | `7df97b135884efbe02a082f8e14667381b2e6c630e3f6245394d64413f9e5fcf` |
| `metrics-k10.json` | `08666764ff0d88779c5c648e4dcea267a851c3be4317499134d5db8da7edb1e3` |
| `positive-inventory.json` | `2dd0086430f8634b9d0c34e281cd338b337e232db4ebb9ab56a3d35faa101789` |
| `comparisons.json` | `b36f17f2881aa33016a166170c434284856799f377625b381cd3ee8e5916c590` |

The manifest also pins the fixture manifest/truth, unchanged scorer/mapping,
and all implementation hashes. Source/qrel/output hashes were rechecked after
the run. Build/typecheck/lint/format passed; JS unit suite **336 passed, one
skipped**, and 13 Python offline Go/qeval/controlled-runner tests passed. No
broad root/integration/e2e/coverage test or independent real-repository Go
qrel run was performed. The separate Engram checkout's unrelated dirty files
were not edited.

**Next decision:** Stop tuning these known questions. If retrieval quality
remains the objective, first author **independently judged, genuinely unseen
Go queries** with explicit negatives/ambiguous implementations on a real
repository, plus a source-identity scoring rule and candidate recall at 10/30.
Only then decide whether a different semantic ranking technique (e.g.
evidence-based query understanding or a separately validated cross-encoder)
merits its own limited trial. Do not expand to other languages from this
failed Go gate.
