# Shape-aware fusion and the embedding-model default (2026-11-22)

Why Sense beat zvec-grep on Java, what it cost to find out, and what the
measured change does to all five quality lanes.

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
- Consequently Sense's `--mode hybrid` equals `--mode semantic` on every one of
  these natural-language queries.

Two differences matter for zvec-grep: the embedding model, and the balance of
the lexical and vector legs during fusion.

## Ablation (Java lane, unchanged fixture, k=10)

| Configuration | nDCG@10 | MRR@10 | recall@10 |
|---|---:|---:|---:|
| potion-code-16m + hybrid (pinned floor) | 0.3250 | 0.4792 | 0.4702 |
| potion-code-16m + vector only | 0.3981 | 0.2917 | 0.6223 |
| all-MiniLM-L6-v2 + hybrid | 0.3710 | 0.3482 | 0.6256 |
| all-MiniLM-L6-v2 + vector only | **0.5347** | **0.6688** | **0.6494** |
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
- The implicit default embedding model is now `local/all-minilm-l6-v2`
  (`pipelines/indexing/service.rs`, `config.rs`), with help text and the
  implicit-index integration tests updated.

Full workspace test suite passes (534 engine unit tests plus the integration
suites).

## Result on the five lanes (shipped configuration)

Run with `benchmarks/quality-lanes/run_qeval_multilanguage.py`, binary sha256
`63dc1850…fd256fc`, checkout `9c32dc2`, two same-root repeats, deterministic on
every lane.

| Lane | nDCG@10 | MRR@10 | recall@10 | precision@10 |
|---|---:|---:|---:|---:|
| go | 0.6359 | 0.7500 | 0.7271 | 0.2750 |
| python | 0.7560 | 0.8438 | 0.8257 | 0.3500 |
| rust | 0.6000 | 0.7750 | 0.6985 | 0.2750 |
| typescript | 0.5749 | 0.5868 | 0.7539 | 0.3000 |
| java | 0.4313 | 0.4220 | 0.6807 | 0.2625 |

Against the pinned floors (`check_baselines_multilanguage.py`):

| Lane | Verdict |
|---|---|
| go | PASS — all four metrics improve |
| python | PASS — all four metrics improve |
| rust | **FAIL** — recall 0.6985 < 0.7423, precision 0.2750 < 0.3000 |
| typescript | **FAIL** — MRR 0.5868 < 0.7083 |
| java | **FAIL** — MRR 0.4220 < 0.4792 |

nDCG improves on all five lanes (+0.002 Rust to +0.117 Python); MRR improves on
three (go, python, rust) and falls on two; recall and precision improve on four
and fall on Rust.

## Why the floors block it

The floors were pinned under potion + lexical-dominant fusion, so each
language's floor encodes a different leg. Measured on the same runs:

- vector only: python recall 0.7632 (< 0.7757) and rust recall 0.7249
  (< 0.7423),
- lexical only: rust recall 0.7110,
- weighted hybrid (1.0 / 2.0): rust recall 0.6985,
- no fusion weight reproduces rust's 0.7423 with MiniLM, because that figure
  comes from potion's vector ranking surfacing units MiniLM misses.

Upper bound check: for Rust, the best possible top-10 drawn from the union of
both legs' lists reaches recall 0.8000, above the floor. Slot-quota fusion was
simulated (keep `10-k` vector hits, fill `k` from the lexical list, k=0..5)
using the stored per-query rankings: the best Rust recall is 0.7230 at k=1,
below 0.7423, and Rust precision falls below 0.3000 for k>=2. A quota cannot
close it either.

So the three failures are a genuine per-language quality trade, not a
bookkeeping artifact. Landing this as the default requires either re-pinning
those three metrics in a reviewed change with the trade documented, or keeping
potion as the default and shipping MiniLM plus shape-aware fusion as an opt-in
(`zg --config model set local/all-minilm-l6-v2 --default`).

## Against Sense

Shipped configuration versus Sense v1.15.1, nDCG@10 / MRR@10:

| Lane | zvec-grep (shipped) | Sense | 
|---|---|---|
| go | **0.6359** / **0.7500** | 0.5763 / 0.7304 |
| python | **0.7560** / **0.8438** | 0.5258 / 0.5938 |
| rust | **0.6000** / 0.7750 | 0.5312 / **0.7812** |
| typescript | **0.5749** / 0.5868 | 0.5164 / **0.6970** |
| java | **0.4313** / 0.4220 | 0.4227 / **0.5458** |

The pure-vector cell (MiniLM + `--vector`) leads Sense on nDCG and MRR in all
five lanes (`.6648/.8750`, `.6417/.8229`, `.6434/.9167`, `.6462/.7639`,
`.5347/.6687`), but fails the python and rust recall floors. The shape-aware
hybrid trades some of that head precision for lexical recall.

## Rejected hypotheses

- **Container header projection (Java)** — projection of every Java container
  to its declaration line: Java nDCG 0.2870. Reverted.
- **Body projection for all Java entities** — 0.2870. Reverted.
- **Indexed-text shape as the cause** — Sense embeds a graph context header,
  but projecting zvec-grep's body text away made Java worse, not better, and
  MiniLM alone recovers more than any text projection.

## Reproduction

```
python3 benchmarks/quality-lanes/run_qeval_multilanguage.py \
  --fixture benchmarks/quality-lanes/fixtures/<lang>-workflow-discovery-v1 \
  --binary rust/target/release/zg --work-root <fresh-root> \
  --output <out> --repeats 2
python3 benchmarks/quality-lanes/check_baselines_multilanguage.py \
  --fixture <lang>-workflow-discovery-v1 --results <out>/metrics.json
```

Ablation cells and simulations: `…/opencode/{lanes-minilm-vec,lanes-minilm-vec2,lanes-potion-vec,lanes-minilm-hyb,lanes-w2-hyb,lanes-ftsonly,shipped-run}/`.
