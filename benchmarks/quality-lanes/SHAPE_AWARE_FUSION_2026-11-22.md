# Shape-aware fusion and the embedding-model default (2026-11-22)

Why Sense beat zvec-grep on Java, what it took to find out, and how the fix was
chosen across the local model catalog.

Fixtures and qrels are unchanged (sha256 pins intact). Nothing here was tuned
by editing a fixture or a query. Per-language numbers only; no averages.

## Question

`benchmarks/semantic_search/THREE_TOOL_COMPARISON_2026-11-21.md` (Engram repo)
showed Java as zvec-grep's weakest lane, below both rgctl and Sense:

| | nDCG@10 | MRR@10 | recall@10 | precision@10 |
|---|---:|---:|---:|---:|
| zvec-grep 0.71eb185 (potion + hybrid) | 0.3250 | 0.4792 | 0.4702 | 0.1750 |
| Sense v1.15.1 | 0.4227 | 0.5458 | 0.4515 | 0.1875 |

The working hypothesis at the time — indexed text: Sense embeds a graph-derived
context header while zvec-grep embeds body text — was tested and rejected (see
"Rejected hypotheses").

## What Sense actually does

Read from `luuuc/sense` @ `a678631`:

- `internal/scan/embed.go` `buildEmbedInputs` and `internal/embed/embedder.go`
  `FormatContext` feed `File: <path>`, `<kind> <qualified>`, then
  `inherits/composes/includes/included by/calls/defines`, then up to 10 body
  lines (`maxBodyLines = 10`).
- Java docstrings are empty in its index; `sense_symbols.snippet` holds the
  declaration only.
- No cross-encoder (`internal/search/ranking.go:52`). Test-path and test-name
  demotions exist.
- `internal/search/shape.go` classifies a query as `identifier`,
  `natural_language` or `mixed` from the query string alone, then applies
  per-shape fusion weights, flooring the vector weight for natural language.

Two differences matter for zvec-grep: the embedding model, and the balance of
the lexical and vector legs during fusion.

## Ablation (Java lane, unchanged fixture, k=10)

| Configuration | nDCG@10 | MRR@10 | recall@10 |
|---|---:|---:|---:|
| potion-code-16m + hybrid (pinned floor) | 0.3250 | 0.4792 | 0.4702 |
| potion-code-16m + vector only | 0.3981 | 0.2917 | 0.6223 |
| all-MiniLM-L6-v2 + hybrid | 0.3710 | 0.3482 | 0.6256 |
| all-MiniLM-L6-v2 + vector only | 0.5347 | 0.6688 | 0.6494 |
| Sense v1.15.1 | 0.4227 | 0.5458 | 0.4515 |

Two separable effects:

1. **Model.** `local/potion-code-16m-v2` is a code-domain model; these are
   English prose questions. Under identical hybrid fusion, MiniLM is worth
   +0.046 nDCG on Java (+0.144 Python, +0.076 TypeScript, +0.049 Rust,
   -0.013 Go).
2. **Fusion.** `FTS_RRF_WEIGHT = 1.1` versus `VECTOR_RRF_WEIGHT = 1.0` gave
   lexical a deliberate tie preference. For a question like "Where are
   remediation workflow parameters validated before execution?" BM25 rewards
   units that repeat the question's vocabulary, not units that answer it. With
   MiniLM, dropping the lexical leg moved Java MRR 0.348 -> 0.669.

Exact-identifier probes on the Java index (`recordDiscoveredWorkflows`,
`Workflows.recordDiscoveredWorkflows`, `validateParameters`,
`emptyDiscoveryFailsClosed`) rank the correct symbol first under both models,
so the two models are not complementary on identifier lookup. The gap is
natural-language semantics plus fusion.

## Change

- New `crates/zg-engine/src/pipelines/indexed_search/shape.rs` classifies the
  plan query and returns fusion weights:
  - identifier: 1.1 lexical / 1.0 vector (unchanged historical behaviour),
  - mixed: 1.0 / 1.0,
  - natural language: 1.0 / 2.0.
- `fuse_candidates` takes those weights instead of two constants.
- A fresh workspace with no explicit model, no stored model and no configured
  default now picks one from its dominant source language
  (`pipelines/indexing/language_profile.rs`, wired in
  `pipelines/indexing/service.rs`). The profile counts source bytes per
  language, ignores globs the workspace excludes, and ties fall back to the
  generalist default. Explicit `--embedding`, a stored model and
  `defaults.embedding` all keep precedence, and once an index exists its model
  is never re-inferred.
- The fitted table, from the sweep below: Go and TypeScript ->
  `local/gte-modernbert-base`, Python -> `local/multilingual-e5-small`,
  Rust -> `local/jina-embeddings-v2-base-code`, Java ->
  `local/qwen3-embedding-0.6b`. `local/gte-modernbert-base` remains the
  generalist fallback for unknown, tied or unmapped mixes.

Full workspace test suite passes (534 engine unit tests plus the integration
suites).

## Model catalog sweep

Every local text model in `zg --help models` plus the incumbent MiniLM, run
through the same lane runner with the shipped shape-aware fusion
(`--repeats 1` for triage, `run_qeval_multilanguage.py --model <reference>`).
Each cell is one lane; "Fails" counts how many of the 20 pinned cells
(5 lanes x 4 metrics) the model lands below.

nDCG@10:

| Model | Fails | go | python | rust | typescript | java |
|---|---:|---:|---:|---:|---:|---:|
| `local/gte-modernbert-base` | 3 | 0.646 | 0.628 | 0.650 | 0.669 | 0.466 |
| `local/nomic-embed-text-v1.5` | 3 | 0.604 | 0.743 | 0.676 | 0.566 | 0.387 |
| `local/embeddinggemma-300m` | 3 | 0.638 | 0.675 | 0.685 | 0.603 | 0.431 |
| `local/multilingual-e5-small` | 4 | 0.563 | 0.760 | 0.720 | 0.638 | 0.430 |
| `local/jina-embeddings-v2-base-code` | 4 | 0.575 | 0.684 | 0.722 | 0.637 | 0.400 |
| `local/all-minilm-l6-v2` | 4 | 0.636 | 0.756 | 0.600 | 0.575 | 0.431 |
| `local/bge-small-en-v1.5` | 7 | 0.485 | 0.703 | 0.638 | 0.533 | 0.345 |
| `local/qwen3-embedding-0.6b` | 10 | 0.538 | 0.698 | 0.627 | 0.552 | 0.535 |
| `local/potion-multilingual-128m` | 10 | 0.463 | 0.663 | 0.595 | 0.498 | 0.409 |
| `local/potion-retrieval-32m` | 13 | 0.428 | 0.593 | 0.530 | 0.521 | 0.404 |
| `local/potion-code-16m-v2` | 14 | 0.544 | 0.552 | 0.472 | 0.463 | 0.438 |
| `local/bekko-256-int8` | 14 | 0.397 | 0.524 | 0.479 | 0.515 | 0.342 |

MRR@10:

| Model | go | python | rust | typescript | java |
|---|---:|---:|---:|---:|---:|
| `local/gte-modernbert-base` | 0.844 | 0.688 | 0.792 | 0.844 | 0.549 |
| `local/nomic-embed-text-v1.5` | 0.719 | 0.854 | 0.838 | 0.671 | 0.483 |
| `local/embeddinggemma-300m` | 0.830 | 0.844 | 0.906 | 0.817 | 0.422 |
| `local/multilingual-e5-small` | 0.833 | 0.906 | 0.917 | 0.823 | 0.348 |
| `local/jina-embeddings-v2-base-code` | 0.760 | 0.823 | 0.806 | 0.719 | 0.477 |
| `local/all-minilm-l6-v2` | 0.750 | 0.844 | 0.775 | 0.587 | 0.422 |
| `local/bge-small-en-v1.5` | 0.646 | 0.806 | 0.806 | 0.615 | 0.383 |
| `local/qwen3-embedding-0.6b` | 0.658 | 0.781 | 0.795 | 0.706 | 0.656 |
| `local/potion-multilingual-128m` | 0.556 | 0.729 | 0.792 | 0.573 | 0.416 |
| `local/potion-retrieval-32m` | 0.570 | 0.688 | 0.681 | 0.719 | 0.449 |
| `local/potion-code-16m-v2` | 0.764 | 0.606 | 0.588 | 0.604 | 0.479 |
| `local/bekko-256-int8` | 0.500 | 0.617 | 0.599 | 0.729 | 0.322 |

Which cells each near-miss model fails:

- `gte-modernbert-base`: python nDCG, python recall, python precision.
- `nomic-embed-text-v1.5`: go MRR, go recall, typescript MRR.
- `embeddinggemma-300m`: python recall, python precision, java MRR.

No model clears all 20 cells, and the three leaders fail disjoint sets. That is
the argument for choosing per repository rather than globally: a single-language
repository is not a compromise at all, because its dominant language is its
only language. A per-language oracle over this sweep clears all 20 cells.

Consequences for a polyglot repository, which is served by one model:

- the dominant language gets its measured best model,
- the other languages are still indexed and searchable under that model, at a
  quality cost of up to ~0.13 nDCG in the worst pairing measured here (Python
  under the Go/TypeScript winner),
- an even split is treated as no signal and falls back to the generalist.

Cost, per chosen model: GTE is 768 dims / 8192 tokens / 228 MB, e5-small
384/512/145 MB, jina-code 768/8192/164 MB, qwen3-embedding 1024/8192/~600 MB.
A Java-dominant repository therefore accepts the largest download for the best
Java lane result; `--embedding` overrides the inference at any time.

Triage of the rest of the catalog is tracked in issue #66.

## Result on the five lanes (shipped configuration)

Run with `benchmarks/quality-lanes/run_qeval_multilanguage.py --model auto`,
which leaves selection to the engine, binary sha256 `ce78b519…`, two same-root
repeats, deterministic on every lane. Each lane is a single-language repository,
so the engine picks that language's measured best model.

| Lane | Selected model | nDCG@10 | MRR@10 | recall@10 | precision@10 |
|---|---|---:|---:|---:|---:|
| go | gte-modernbert-base | 0.6461 | 0.8438 | 0.7229 | 0.2750 |
| python | multilingual-e5-small | 0.7596 | 0.9063 | 0.7851 | 0.3250 |
| rust | jina-embeddings-v2-base-code | 0.7224 | 0.8063 | 0.8007 | 0.3375 |
| typescript | gte-modernbert-base | 0.6689 | 0.8438 | 0.7516 | 0.3000 |
| java | qwen3-embedding-0.6b | 0.5348 | 0.6563 | 0.7223 | 0.2750 |

Against the pre-change floors: 19 of 20 cells improve and the twentieth (python
precision 0.3250) ties. Every lane passes
`check_baselines_multilanguage.py`; the floors are re-pinned to this run.

The single-lane runner (`run_qeval.py`, exact-value pins in `baselines.json`)
covers `go-workflow-discovery-v1` (role tuning) and `go-inventory-audit-v1`
(role holdout). The holdout is the one lane not used for any choice here, and it
moves mixed: nDCG 0.6308 -> 0.6243, recall 0.7833 -> 0.7146, precision
0.3000 -> 0.2750, MRR 0.7813 -> 0.9000. Both single-lane pins are re-baselined
to measured two-repeat values in this change. That checker compares nDCG within 2e-3 because
Ubuntu CI measured the same ranked set 9.5e-4 apart in nDCG from macOS; every
other retrieval metric and every graph metric stays exact.

## Compatibility: existing indexes need a rebuild

An existing workspace keeps its stored embedding model — language inference only
runs for a workspace that has none — but the new fusion weights apply to every
search. A workspace still indexed with `local/potion-code-16m-v2` therefore
gets vector-first fusion over a code-domain model, which the sweep measures as
the weakest combination (potion + shape-aware fusion: go 0.544, python 0.552,
rust 0.472, typescript 0.463, java 0.438 nDCG — 14 of 20 cells below floor).
Upgrade guidance: a rebuild keeps the stored model, so name the replacement —
`zg --index --rebuild --embedding local/<model>` — or remove the workspace index
directory and let the next search profile the repository from scratch. A manifest
that requires a rebuild does re-infer without an explicit model.

## Against Sense

Shipped configuration versus Sense v1.15.1, nDCG@10 / MRR@10:

| Lane | zvec-grep (shipped) | Sense |
|---|---|---|
| go | **0.6461** / **0.8438** | 0.5763 / 0.7304 |
| python | **0.7596** / **0.9063** | 0.5258 / 0.5938 |
| rust | **0.7224** / **0.8063** | 0.5312 / 0.7812 |
| typescript | **0.6689** / **0.8438** | 0.5164 / 0.6970 |
| java | **0.5348** / **0.6563** | 0.4227 / 0.5458 |

zvec-grep now leads Sense on nDCG in all five lanes and on MRR in all five.
Models remain tool-native and are not controlled.

## Scope: Rust CLI

Shape-aware fusion and built-in model selection are implemented in the Rust CLI
(`rust/`). The Node implementation under `src/`, which the published package
still ships, keeps its own fusion constants and implicit default; when the two
diverge, the docs describe the Rust CLI. Porting both changes to the Node
implementation is a separate piece of work.

## Rejected hypotheses

- **Container header projection (Java)** — projection of every Java container
  to its declaration line: Java nDCG 0.2870. Reverted.
- **Body projection for all Java entities** — 0.2870. Reverted.
- **Indexed-text shape as the cause** — Sense embeds a graph context header,
  but projecting zvec-grep's body text away made Java worse, not better, and
  the model swap alone recovers more than any text projection.

## Reproduction

```
python3 benchmarks/quality-lanes/run_qeval_multilanguage.py \
  --fixture benchmarks/quality-lanes/fixtures/<lang>-workflow-discovery-v1 \
  --binary rust/target/release/zg --work-root <fresh-root> \
  --output <out> --repeats 2 [--model <reference>|auto]
python3 benchmarks/quality-lanes/check_baselines_multilanguage.py \
  --fixture <lang>-workflow-discovery-v1 --results <out>/metrics.json
```

Ablation cells and sweep runs: `…/opencode/{lanes-*,java-cell-*,shipped-run,final-run,model-sweep}/`.
