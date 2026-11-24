# Fusion recall trace: where the lexical-only relevant units go (2026-11-23)

Issue #68 spike: keep lexical-only relevant units without displacing the
vector head. This report answers the issue's unknowns with measured data,
adds four never-tuned-on holdout fixtures, and reports the measured frontier
of fusion candidates. Per the issue's acceptance clause, no candidate met
every floor, so no engine change ships by default; the frontier and the
smallest trade are reported for a maintainer decision.

Fixtures and qrels of the six pre-existing lanes are byte-identical
(sha256 pins intact). Nothing was tuned by editing a fixture, a query or a
qrel. Per-language numbers only; no averages.

## Tools introduced (both hash-pinned lane assets)

- `trace_route_diff.py` — per-query route trace on any lane fixture: runs
  `--hybrid --limit 10 --trace` (the lane's measured configuration) plus
  `--fts`/`--vector` at `--limit 20` on one frozen index, maps hits to
  manifest unit ids, and reports for every recall-relevant unit missing from
  the hybrid top-10 its rank in each single route, the `matchedBy` label and
  the engine's per-route recall trace. Its hybrid top-10 was validated
  byte-identical to the lane runner's `normalized-runs.json` on
  `go-inventory-audit-v1` (8/8 queries).
- `replay_fusion.py` — offline replay of the fusion stage over collected
  engine traces. `candidate_fusion_score` is a line-for-line Python port of
  `rust/crates/zg-engine/src/pipelines/indexed_search/pipeline.rs`
  (constants included; the unit tests port the Rust property tests with
  their exact-value assertions).

### Replay fidelity contract

The replay's baseline must reproduce every pinned lane metric exactly
before any variant number is believed. Measured (binary sha256
`cc13305a…`, model auto, macOS/arm64):

| Fixture | baseline replay vs pinned floor |
|---|---|
| go-workflow-discovery-v1 | identical (all four metrics) |
| python-workflow-discovery-v1 | identical |
| rust-workflow-discovery-v1 | identical |
| typescript-workflow-discovery-v1 | identical |
| java-workflow-discovery-v1 | identical |
| go-inventory-audit-v1 | identical |
| python/rust/typescript/java-inventory-audit-v1 | identical to the two-repeat pinning runs below |

Two fidelity details matter for anyone reusing the harness:

1. The engine's own deep run (`--limit 200 --trace`) orders hits by fused
   scores computed **with the deep limit** — the corroboration band test is
   `max_rank <= limit`, so the band widens with the limit and the deep
   ordering differs from the lane (limit 10) ordering. The replay therefore
   rescores **every hit (entity)** at the lane limit and only then selects
   and maps to units with the runner's first-seen dedup. Deduplicating by
   unit id before rescoring picks the wrong entity whenever a unit has
   several entities (Rust impl blocks vs struct declarations).
2. A wrong-weights replay is a good canary: keying the route weights on the
   plan route id instead of the resolved `fts`/`vector` route ids silently
   falls back to the flat identifier weights (1.1/1.0) — and the replay then
   reproduced the previously measured gte + old-fusion 2x2 cell
   (.6611/.7708/.7833/.3000) exactly, an independent confirmation that the
   scoring port is faithful.

## Unknown 1: what exactly is lost (go-inventory-audit-v1)

Nine recall-relevant units (grade >= 2; the scorer counts recall@10 over
grade >= 2 only — verified against the pinned 0.7146) are missing from the
hybrid top-10 across 7 of 8 queries. Their ranks in the single routes:

| Missing unit (grade) | lexical top-20 | vector top-20 |
|---|---:|---:|
| `list_assets.go::RecordScanned` (3) | — | 20 |
| `enforcer.go::Enforcer.CheckAccess` (3, deny query) | 4 | 13 |
| `state.go::AssetState.Add` (2) | 3 | 20 |
| `state.go::AssetState.Contains` (2, recheck) | — | 18 |
| `state.go::AssetStateFromContext` (2) | — | — |
| `enforcer.go::AccessDenied` (2) | — | 14 |
| `enforcer.go::Enforcer.CheckAccess` (2, enforce query) | 3 | 9 |
| `state.go::AssetState.Contains` (3, empty-scan) | — | 11 |
| `registry.go::AssetRegistry.GetAsset` (2) | 14 | — |

Tally: 7 of 9 sit in the vector-only top-20 (ranks 9-20), 4 in the
lexical-only top-20, 2 in both, 1 (`AssetStateFromContext`) in neither —
that one is unrecoverable by any fusion change and caps the achievable
recall gain. Only 1 unit is reachable solely through the lexical route
(`AssetRegistry.GetAsset`, lexical rank 14). The loss is not "lexical
finds the answer and fusion drops it"; it is "both routes see most answers,
but mid-ranked".

Full per-query tables incl. `matchedBy` and engine traces:
`trace_route_diff.py` output (see Reproduction).

## Unknown 1 (mechanism): why fusion drops them

`candidate_fusion_score` does not sum weighted RRF. It takes the best
per-mode weighted contribution as primary and adds the other route as
corroboration — capped at 25% of the primary when both routes' ranks sit in
the same band, but multiplied by **0.07** (`DISCORDANT_ROUTE_RRF_WEIGHT`)
when the ranks straddle the band boundary (`max_rank > limit`).

Worked example, `Enforcer.CheckAccess` on the enforce query (lexical rank 3,
vector rank 9, natural-language weights 1.0/2.0, limit 10). Both ranks sit
within the shallow band (9 <= 10), but the agreement test also requires the
ranks to be close: `max_rank <= 2.5 x min_rank`, and 9 > 7.5, so the routes
count as discordant. Primary = vector 2.0/69 = .0290; the lexical rank-3
evidence is discounted to 0.07 x 1.0/63 = .0011. Total .0301 (the replay
reproduces this score exactly) — below the tenth-placed hit. On the deny
query the pair (lexical 4, vector 13) additionally straddles the band
(13 > 10). Under the natural-language vector weight 2.0, corroborated
lexical-rank-3/4 finds lose to a stack of vector-only rank 1-8 hits. That
is the displacement mechanism, and it explains why lowering the vector
weight to 1.5/1.25 did nothing (the issue's prior measurement): the
relative weight is not what removes the units — the discordant-route
discount (0.07, triggered by either the band or the 2.5x closeness test)
is.

## Unknown 2: four new holdout fixtures

`python-inventory-audit-v1`, `rust-inventory-audit-v1`,
`typescript-inventory-audit-v1`, `java-inventory-audit-v1`: domain-shifted
(inventory audit / permission enforcement; zero workflow-discovery
vocabulary), same 8 query archetypes as the Go holdout, source-authored
truth expanded to complete-universe qrels by the vendored
`tools/build_synthetic_qrels.py` (from jordigilh/engram, byte-identity
tested against three committed lanes). 43-45 units each, `unmatched_hits:
0`, deterministic. **Never tuned on**: no fusion choice in this spike used
them for selection before the final frontier measurement.

Baseline (shipped engine, model auto, two repeats, deterministic):

| Fixture | nDCG@10 | MRR@10 | recall@10 | precision@10 |
|---|---:|---:|---:|---:|
| python-inventory-audit-v1 | 0.4174 | 0.4792 | 0.5646 | 0.2125 |
| rust-inventory-audit-v1 | 0.5580 | 0.6771 | 0.7000 | 0.2625 |
| typescript-inventory-audit-v1 | 0.3684 | 0.5000 | 0.5854 | 0.2250 |
| java-inventory-audit-v1 | 0.3759 | 0.5156 | 0.5396 | 0.2000 |

These are pinned in `baselines-multilanguage.json` with `"role":
"holdout"` and gated by `check_baselines_multilanguage.py` in CI, so any
future fusion change is judged on 10 fixtures, not 6. Two holdouts
measured differently on the ubuntu-24.04 CI runner (inference drift, the
same phenomenon PR #67 recorded for the java lane): rust-inventory-audit-v1
measured below macOS on all four metrics and java-inventory-audit-v1 on
MRR only, so those floors are pinned to the lower ubuntu values with the
macOS measurements recorded in `floor_exception` notes — the same
cross-platform-minimum policy the java lane floor already uses.

## Unknown 5: does the loss correlate with qrels density

Counting distinct grade>=2 units per query: the Go holdout averages 3.88
(range 3-5), the five lanes 3.88-4.38 (range 1-8), and the four new
holdouts 3.88 (range 3-5) — no meaningful density difference. The new
holdouts show recall losses of the same class under the shipped engine, so
the pattern is not Go-specific and not density-driven.

## The measured frontier (all 10 fixtures, offline replay)

Candidates on the two knobs the trace identified — the discordant-route
discount (`DISCORDANT_ROUTE_RRF_WEIGHT`, shipped 0.07) and the rank-closeness
test (`ROUTE_RANK_AGREEMENT_FACTOR`, shipped 2.5x) — plus the band-widening
alternative. "Fails" lists every fixture-metric below its pinned floor
(nDCG tolerance 2e-3; MRR/recall/precision exact):

| Variant | Go holdout (nDCG/recall) | Fails |
|---|---|---|
| 0.07 (shipped) | 0.6243 / 0.7146 | — (the status quo; holdout below its pre-#67 values) |
| band2 (band edge limit -> 2*limit) | 0.5736 / 0.6521 | holdout nDCG -5.1e-2, recall -6.3e-2 |
| 0.10 | 0.6250 / 0.7146 | java lane MRR -6.25e-3; python holdout nDCG -3.5e-3; **does not recover the holdout** |
| 0.12 | 0.6353 / 0.7458 | java lane MRR -6.25e-3; python holdout nDCG -2.9e-3; rust holdout nDCG -8.0e-3, recall -1.04e-2 |
| **0.15** | **0.6358 / 0.7458** | python holdout nDCG -2.9e-3; rust holdout MRR -6.25e-3 |
| 0.25 | 0.6424 / 0.7458 | go lane nDCG -1.16e-2, recall -2.5e-2 |
| agreement 3.0 (closeness 2.5x -> 3x) | 0.6134 / 0.7458 | rust lane nDCG -4.7e-3; go holdout nDCG -1.09e-2; java holdout MRR -5.2e-3 |
| agreement 4.0 (closeness 2.5x -> 4x) | 0.6047 / 0.7458 | python lane nDCG -9.0e-3; java lane MRR -6.25e-3; go holdout nDCG -1.96e-2, MRR -6.25e-2 |

`discordant:0.15` is the smallest trade on the frontier. Its full per-fixture
effect (vs pinned floors): go holdout improves on all four metrics
(.6358/.9000/.7458/.2875 — no downward re-pin needed); rust, typescript and
java lanes improve on nDCG/recall/precision with MRR equal; go and python
lanes move only within the nDCG epsilon (-3.4e-5, -4.5e-4, other metrics
equal); python holdout loses 2.9e-3 nDCG (recall/MRR/precision equal) and
rust holdout loses 6.25e-3 MRR (other metrics improve).

The frontier is non-monotonic (0.12 is worse than 0.15 on the rust
holdout): the knob moves discrete ranking flips, not a smooth curve, which
is also why no interpolation "between" values is guaranteed to exist.

## Conclusion

No candidate meets the issue's acceptance (every fixture at or above its
pinned floor). Per the acceptance clause, the measured frontier and the
smallest trade are reported and **no engine change ships by default**. The
decision for the maintainer: accept `discordant:0.15`'s two sub-tolerance-
adjacent costs (python-holdout nDCG -2.9e-3, rust-holdout MRR -6.25e-3) to
recover the Go holdout fully, or keep 0.07. The replay harness makes either
future path cheap to re-evaluate on all 10 fixtures before touching Rust.

## Incidental finding (pre-existing, not fixed)

`java-workflow-discovery-v1`'s committed `qrels.json` snapshot digest does
not match its current sources under the builder's convention (same 9 files
and byte total, different content digest) — the qrels were built from a
slightly different source state than what is vendored. CI cannot see this
(`check_multilanguage_fixtures.py` compares qrels snapshot vs the
baselines pin, not vs the files). Recorded in
`test_build_synthetic_qrels.py` as a pinned known state; fixing it means
re-generating java qrels in a reviewed change, which this spike
deliberately does not do.

## Reproduction

```bash
# route trace on the Go holdout
python3 benchmarks/quality-lanes/trace_route_diff.py \
  --fixture benchmarks/quality-lanes/fixtures/go-inventory-audit-v1 \
  --binary rust/target/release/zg --work-root <fresh-root> \
  --output <fresh-out> --model auto

# fusion replay (baseline must reproduce the pinned metrics exactly)
python3 benchmarks/quality-lanes/replay_fusion.py \
  --fixture benchmarks/quality-lanes/fixtures/go-inventory-audit-v1 \
  --binary rust/target/release/zg --work-root <fresh-root> \
  --output <fresh-out> --model auto \
  --variants baseline,discordant:0.15

# unit tests (shape port, qrels builder, replay port)
python3 -m unittest discover -s benchmarks/quality-lanes -p 'test_*.py'
```

Frontier runs: `…/opencode/{replay-frontier,replay3b,replay-newhold}/`.
