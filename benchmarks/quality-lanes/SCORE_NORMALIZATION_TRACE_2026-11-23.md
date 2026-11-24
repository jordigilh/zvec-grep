# Per-route score normalization: measured frontier (2026-11-23)

Issue #70 spike: replace the rank-band RRF fusion's corroboration gating
with per-route score normalization (min-max, z-score, rank+raw hybrid),
so corroboration no longer depends on limit-dependent, discontinuous rank
bands. Per the issue's acceptance clause, **no candidate met every pinned
floor**, so no engine change ships: the measured frontier is reported
instead — the same sanctioned fallback as #68, and this family's smallest
trade is strictly worse than the rank-band frontier's.

Fixtures, queries and qrels are byte-identical (sha256 pins intact).
Nothing was tuned by editing a fixture, a query or a qrel. Per-language
numbers only; no averages.

## Fidelity contract, re-validated

`replay_fusion.py` gained per-query normalization variants and a trace
dump/reuse mode (`--dump-traces` / `--traces-from`) so variants iterate on
one collected trace set without re-running the engine. Before any variant
number was believed, the baseline replay was re-validated on **all ten
fixtures** with the same pinned binary as the #69 frontier
(sha256 `cc13305a…`, model auto, macOS/arm64): every pinned lane metric
reproduced exactly, including the two holdouts whose floors are pinned to
the lower ubuntu-24.04 values (their macOS measurements match the values
recorded in `floor_exception`).

## Key finding: the vector route's trace score is a distance, not a similarity

Issue #70's premise — "per-route raw scores … fts BM25-style and vector
cosine" as directly fusable similarities — is half wrong, and the wrong
half inverts a route:

- The fts route's `SearchRecallTrace.score` is BM25 (higher is better).
- The vector route's score is whatever the fp16 index returns for
  `MetricType::Cosine` (`Fragments::search_vector` passes
  `doc.get_score()` through, `rust/crates/zg-engine/src/storage/fragments/mod.rs`),
  and on these fixtures it is a **distance: lower is better**. Measured on
  `go-inventory-audit-v1` (query 1): vector rank 1 carries the lowest
  score (0.2524) and rank 44 the highest (0.5546) — the score rises
  monotonically with rank.

Treating the vector score as a similarity therefore **inverts the vector
route**: the first polarity-naive run measured nDCG@10 of 0.010–0.151
across the ten fixtures (recall@10 down to 0.031). The replay now derives
per-route polarity from the rank/score covariance over the candidate set
(`route_score_stats`), which detects the flip per query without hardcoding
it; all numbers below are polarity-corrected. Any future consumer of these
trace scores must respect this polarity.

## Variants

All variants normalize per query over the deep candidate set, fuse the two
routes' weighted normalized contributions, and apply **no rank-band test**
(no `limit` band, no 2.5x closeness test):

| Variant | Rule |
|---|---|
| `norm:minmax` | sum of weight x min-max-normalized raw score per route |
| `norm:zscore` | sum of weight x squashed z-score (0.5 + 0.5·z/(1+\|z\|)) per route |
| `norm:hybrid[:alpha]` | per route: alpha x min-max raw + (1-alpha) x reciprocal rank rescaled to (0,1] |
| `norm:capped[:ratio]` | like `norm:minmax`, but best route is primary and the other route corroborates capped at `ratio` x primary (default 0.25 — the shipped fusion's head protection, minus its rank bands) |

## The measured frontier (all 10 fixtures, offline replay)

"Fails" lists every fixture-metric below its pinned floor (nDCG tolerance
2e-3; MRR/recall/precision exact). The go holdout's floors are pinned in
`baselines.json`; the other nine in `baselines-multilanguage.json`.

| Variant | Fails (count) | Failing fixture-metrics |
|---|---:|---|
| baseline (shipped) | 0 | — (status quo) |
| norm:minmax | 14 | go-lane recall -2.5e-2, precision -1.25e-2; python-lane nDCG -2.0e-3, MRR -5.2e-2; ts-lane MRR -6.3e-2; java-lane nDCG -5.1e-2, MRR -1.9e-1; go-holdout nDCG -4.0e-2, MRR -2.3e-1; ts-holdout recall -1.5e-2, precision -1.25e-2; java-holdout nDCG -3.5e-2, recall -3.1e-2, precision -1.25e-2 |
| norm:zscore | 11 | go-lane recall -2.5e-2, precision -1.25e-2; python-lane nDCG -1.3e-2, MRR -6.3e-2; java-lane nDCG -6.0e-2, MRR -2.2e-1; go-holdout nDCG -6.9e-2, MRR -2.3e-1; java-holdout nDCG -7.1e-2, recall -9.8e-2, precision -3.75e-2 |
| norm:hybrid (alpha .5) | 11 | go-lane recall -2.5e-2, precision -1.25e-2; python-lane nDCG -4.9e-3, MRR -5.2e-2; java-lane nDCG -4.3e-2, MRR -1.6e-1; go-holdout nDCG -4.5e-2, MRR -2.3e-1; java-holdout nDCG -3.0e-2, recall -3.1e-2, precision -1.25e-2 |
| norm:hybrid:0.25 | 9 | go-lane recall -2.5e-2, precision -1.25e-2; java-lane nDCG -4.6e-2, MRR -1.6e-1; go-holdout nDCG -6.1e-2, MRR -2.1e-1; java-holdout nDCG -3.5e-2, recall -3.1e-2, precision -1.25e-2 |
| norm:hybrid:0.75 | 11 | go-lane recall -2.5e-2, precision -1.25e-2; python-lane nDCG -4.1e-3, MRR -5.2e-2; java-lane nDCG -4.3e-2, MRR -1.6e-1; go-holdout nDCG -4.3e-2, MRR -2.3e-1; java-holdout nDCG -3.4e-2, recall -3.1e-2, precision -1.25e-2 |
| **norm:capped (0.25)** | **8** | python-lane recall -3.1e-2, precision -1.25e-2; ts-lane MRR -5.2e-2; java-lane nDCG -2.0e-2, MRR -1.0e-1, recall -1.3e-2; go-holdout nDCG -5.1e-2, MRR -1.8e-1 |
| norm:capped:0.15 | 10 | go-lane MRR -6.25e-3; python-lane recall -4.7e-2, precision -2.5e-2; ts-lane nDCG -2.4e-2, MRR -5.2e-2; java-lane nDCG -5.4e-3, MRR -4.2e-2, recall -1.3e-2; go-holdout nDCG -6.2e-2, MRR -2.1e-1 |
| norm:capped:0.35 | 11 | go-lane recall -2.5e-2, precision -1.25e-2; python-lane nDCG -1.1e-2, recall -3.1e-2, precision -1.25e-2; ts-lane MRR -5.2e-2; java-lane nDCG -3.9e-2, MRR -1.7e-1, recall -3.0e-3; go-holdout nDCG -2.8e-2, MRR -1.7e-1 |

The best family member, `norm:capped` (nDCG/MRR/recall/precision per
fixture: go-lane .7009/.9062/.7229/.2750, python-lane
.7817/1.0000/.7539/.3125, rust-lane .7708/.9375/.8030/.3375, ts-lane
.7069/.7917/.8007/.3250, java-lane .5147/.5521/.7089/.2750, go-holdout
.5735/.7188/.7458/.2875, python-holdout .4554/.6429/.5646/.2125,
rust-holdout .6012/.8333/.7771/.3000, ts-holdout .4370/.6042/.6167/.2375,
java-holdout .4109/.5615/.5708/.2125), still fails 8 fixture-metrics —
four times the rank-band frontier's smallest trade (`discordant:0.15`,
2 failing fixture-metrics, PR #69).

## Mechanism: why normalization loses

1. **Raw-score magnitudes are less calibrated than rank positions.** The
   routes' rank order is their own quality ordering, but the magnitude
   spread is not comparable across queries: BM25's dynamic range varies
   with term statistics (one exact-identifier match can score 12.2 while
   the next hit scores 3.4), and cosine distances compress
   (0.25–0.55 on the go holdout). Min-max stretching amplifies exactly
   those magnitude differences, which carry little relevance signal.
2. **Head displacement.** Continuous corroboration (a sum, even capped at
   0.25 of the primary) systematically displaces single-route head hits:
   the go holdout loses MRR -1.7e-1 to -2.3e-1 under every variant, and
   the go lane loses recall -2.5e-2 under every variant except `norm:capped`
   and `norm:capped:0.15`. The recall gains the redesign targets do appear
   (go-holdout recall .7146 -> .7458–.8083, python-holdout .5646 -> .7208)
   but are bought at the head's expense — the opposite end of the ranking
   the MRR/nDCG floors pin.
3. **The java lane is the canary.** Its MRR collapses under the whole
   family (-4.2e-2 to -2.2e-1), and it was already the fixture that
   exposed the #68 frontier's limits.

## Conclusion

Per the acceptance clause, **no engine change ships**: no normalization
variant comes close to clearing every pinned floor on all ten fixtures,
and the family's best (`norm:capped`, 8 failing fixture-metrics) is
strictly worse than the #68 rank-band frontier's smallest trade
(`discordant:0.15`, 2). The maintainer decision from PR #69 is therefore
unchanged by this alternative: keep the shipped fusion (0.07) or accept
`discordant:0.15`'s two sub-tolerance-adjacent costs. The durable outputs
of this spike are the polarity finding above (a prerequisite for any
future raw-score work) and the replay harness's dump/reuse mode, which
makes the next fusion candidate measurable on all ten fixtures in minutes.

## Reproduction

```bash
# collect once (pinned binary cc13305a…, model auto), then iterate:
python3 benchmarks/quality-lanes/replay_fusion.py \
  --fixture benchmarks/quality-lanes/fixtures/go-inventory-audit-v1 \
  --binary rust/target/release/zg --work-root <fresh-root> \
  --output <fresh-out> --model auto --dump-traces \
  --variants baseline,norm:minmax,norm:zscore,norm:hybrid,norm:capped

python3 benchmarks/quality-lanes/replay_fusion.py \
  --fixture benchmarks/quality-lanes/fixtures/go-inventory-audit-v1 \
  --traces-from <fresh-out> --output <fresh-out-2> \
  --variants norm:capped,norm:capped:0.15,norm:capped:0.35

# unit tests (fusion port + normalization variants, incl. polarity)
python3 -m unittest discover -s benchmarks/quality-lanes -p 'test_*.py'
```

Frontier runs (ephemeral session scratch, purged by macOS; re-collect with
the commands above): `…/opencode/norm70/{out,replay,grid}-*/` (work roots
`root-*`, unique basenames). The dumped trace set at
`norm70/out-go-inventory-audit-v1/traces.json` holds the per-route raw
scores used for the polarity analysis.
