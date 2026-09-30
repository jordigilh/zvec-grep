# SWE-QA20 retrieval baseline (2026-09-30)

## Result

The full frozen retrieval-only protocol completed successfully against the
current release package: 20 questions, 11 pinned repositories, three modes,
five repetitions, and 300 successful MCP calls. The local CPU embedding model
was `local/potion-code-16m-v2`; the candidate was built from the current
worktree at HEAD `1153a05`.

| Mode | File Hit@1 | File Hit@5 | File Hit@10 | MRR@10 | Repository-macro nDCG@10 | Stable Top 10 | Avg / P50 RT |
|---|---:|---:|---:|---:|---:|---:|---:|
| `zg-hybrid` | 0.3500 | 0.5500 | 0.6500 | 0.4550 | **0.3527** | 20/20 | 69.4 / 33.9 ms |
| `zg-fts` | 0.2000 | 0.5500 | **0.7000** | 0.3661 | 0.2996 | 20/20 | 37.4 / 30.7 ms |
| `zg-vector` | **0.4000** | 0.5500 | 0.6000 | **0.4558** | 0.3254 | 20/20 | 3.3 / 3.0 ms |

Mean public response sizes were 5.685 KiB for hybrid, 5.647 KiB for FTS, and
5.291 KiB for vector. The report passed integrity validation with no product
errors.

## Interpretation

- Hybrid has the best repository-macro nDCG: 8.4% above vector and 17.7% above
  FTS on this suite. This is the most useful headline result for the current
  protocol.
- Vector places a relevant file first most often and has marginally higher MRR;
  FTS finds the most labeled files somewhere in the Top 10 but ranks them less
  effectively.
- These are **file-localization regression signals**, not answer-quality
  scores. A result receives credit when its public path matches one of 39
  accepted targets; the labels are partial positives and are not independently
  blind-validated. The suite does not establish complete recall, evidence
  sufficiency, or agent answer correctness.

The numbers are not directly comparable with BEIR, Sense qevals, or another
semantic-retrieval system unless the other system uses the same frozen
questions, repositories, target labels, chunking, model, route protocol, and
aggregation. The earlier Sense-improvement qevals are therefore complementary
controlled experiments, not a baseline that can be numerically merged with
this report.

For now, treat this run as an acceptable **monitoring baseline**, not as proof
of external competitiveness. The protocol intentionally keeps the quality gate
report-only. Retrieval/index/ranking changes should compare against these
values using the same protocol; the Graphify/codegraph call-edge work should
not be gated on them because graph extraction is not part of these three search
arms.

## Evidence

The validated raw report and per-call evidence are outside the repository at:

`/private/var/folders/r7/gktmmltd1zq7wqhsjjslwsm80000gn/T/zg-sweqa20-release-20260930/retry-portfix/results/`

The run used a temporary harness-only `--listen` addition to isolate each MCP
server from an existing process on port 7999; no repository files were changed
for that workaround.
