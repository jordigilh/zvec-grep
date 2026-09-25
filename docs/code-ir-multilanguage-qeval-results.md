# Code IR projection qevals: four independent language lanes (2026-09-25)

## Summary

The same-engine Code IR projection ablation has now been run against all four
frozen Engram source-unit qevals. Results are lane-specific; do **not** average
or pool the independent corpora/qrels.

| Language | Syntax-only nDCG / MRR / Recall / Precision @10 | Code IR projection nDCG / MRR / Recall / Precision @10 | Aggregate gate |
|---|---|---|---|
| Go | 0.588774 / 0.733333 / 0.677083 / 0.250000 | 0.489163 / 0.718750 / 0.543750 / 0.200000 | **Fail** — all four regress |
| Python | 0.546521 / 0.547917 / 0.689583 / 0.250000 | 0.492982 / 0.531250 / 0.622917 / 0.225000 | **Fail** — all four regress |
| Rust | 0.416330 / 0.455556 / 0.560417 / 0.200000 | 0.441125 / 0.465774 / 0.622917 / 0.225000 | **Pass aggregate gate** — all four improve |
| TypeScript | 0.426791 / 0.591667 / 0.550000 / 0.200000 | 0.460006 / 0.600000 / 0.616667 / 0.225000 | **Pass aggregate gate** — all four improve |

The acceptance rule is evaluated independently for each language: at least one
aggregate metric must improve and none of the other three may decline, followed
by per-query review. Rust and TypeScript meet the aggregate rule but still have
individual query losses; neither result justifies global/default enablement.
Go and Python do not meet the rule.

## Per-query losses to retain in follow-up

- **Go:** all four aggregates fall. `reject-undiscovered-workflow` returns no
  relevant Code IR projection unit at @10, where the syntax-only control returns
  `Validator.IsAllowed` in the top ten.
- **Python:** all four aggregates fall. `preserve-membership-on-retry` loses all
  relevant units at @10 in the Code IR arm.
- **Rust:** aggregate pass, but `preserve-membership-on-retry` loses all relevant
  units at @10; `validate-workflow-parameters` also loses nDCG/MRR.
- **TypeScript:** aggregate pass, with small nDCG declines on
  `forward-labels-to-discovery` and `interactive-selection-guard`; other query
  deltas are neutral or positive.

Full per-query, per-category and raw-rank data is preserved in each run's
`metrics-k10.json`, `comparison.json`, `normalized-runs.json` and
`raw-runs.json` artifacts.

## Reproducibility and scope

- Go: 38 units / 10 files; source SHA-256
  `94183663d9e5f81b753dde7c460428c2414c46867b936b5cc42f9827dab0827f`; qrels
  SHA-256 `0d89687ffd9aac8e6688e9c0931aeb27cca55b99d04f6379ecc5b9bba541c606`.
- Python: 33 units / 8 files; source SHA-256
  `0b77b06131e0a0e8ffbafa258c54e8900b7c32c2ea6f156ea75e8c6a181e076b`; qrels
  SHA-256 `5daba6da79055aba67d8a7cf8f866b1662f89cde905da3703c08f06975a3b279`.
- Rust: 39 units / 14 files (including the six grade-0 module groups); source
  SHA-256 `eb8b41c271aa13160a1ba7ce63af7c8ea35461f3f1bfa8248a3845d19d44c98a`;
  qrels SHA-256 `b6c63eb8b7741ec22d1dd7f66e71489315a803fd4d2c083e95ca2d092db262b9`.
- TypeScript: 33 units / 8 files; source SHA-256
  `152a5562e1fbc0a44f8e7143fa29b9559c6a1ad4a5b85c309a092bb9814b1057`; qrels
  SHA-256 `af738283d787b2198c2e125cfd5346f4aa88a05d89ecf4926e84821f670d936e`.

Each comparison uses fresh indexes from Code IR worktree
`139fe4e7eda0c084e85eb641003a2e9204a87c60`, zvec 0.7.0, CPU and
`local/potion-code-16m-v2`; only the indexing representation changes within a
language pair. The Code IR arm is an evaluation-only adapter that indexes
source-mapped projection windows and queries through the same zvec FTS/vector
search and fusion implementation. It does not modify production search or
consume the sidecar at runtime.

| Language | Code IR snapshot | Projection units searched | Metrics SHA-256 |
|---|---|---:|---|
| Go | `38924ec8adb36ec929b2a9ac01f3ec09ce786fe8d355fad98e17a42507dc6be6` | 58 | `7632bdb7bd70d24fa55f5c21d3c0dc801347118434d0ff2f86586b67bcf8e776` |
| Python | `b901537377569636fd47164dde23be1a915e3dd31553a051ad37c9666c20cc01` | 45 | `ce6fa553cbe32ad157af61af570b36af32d948625cf9b3dad62d25078d910c89` |
| Rust | `d5b98f703a6c3966f730d8bfdec80ed7fef07133aadd91640df727c73fc6ef94` | 49 | `1c764ac0bb67351fda94006d289fee4824c5b59ddefc52108a194f08fb3b10c9` |
| TypeScript | `6b2fa36b2e6d16f7ca97661ceadca9c3b01ba9bd6228a1f7ad96a328cca6e3b2` | 38 | `d0377ec437fe5889bef14b06a504dfe40556f4e6525c31c35d465582a78930dc` |

This four-lane matrix evaluates **Code IR projections**. The additional
SCIP-reference-text ablation is currently Go-only because Go is the producer
change under active upstream review; see
[`go-ir-qeval-results.md`](./go-ir-qeval-results.md). Separate, per-producer
SCIP qeval arms for Python, Rust and TypeScript remain next.

Go's independent AST inventory confirms exact spans/kinds for its 38/38 qeval
units. The other three runs use their frozen manifest line spans for qeval
normalization; their independent exact byte-span/kind inventories still need
completion before those lanes are eligible for promotion, regardless of these
retrieval scores.
