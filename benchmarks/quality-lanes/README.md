# Quality lanes

Retrieval-quality and graph-quality regression gates for the fork. Implements
[jordigilh/zvec-grep#49](https://github.com/jordigilh/zvec-grep/issues/49).

Two lanes run on every PR whose diff touches their trigger paths (see
[`.github/workflows/quality-lanes.yml`](../../.github/workflows/quality-lanes.yml)):

| Lane | Gates | Trigger paths (in addition to `benchmarks/quality-lanes/**`) |
|---|---|---|
| `retrieval-quality` | hybrid semantic-search retrieval metrics vs adjudicated qrels | `rust/crates/zg-engine/**`, `src/engine/**`, `src/index.ts`, `src/index-progress.ts`, `rust/Cargo.lock`, `rust/rust-toolchain.toml` |
| `graph-quality` | native codegraph call-edge quality vs compiler truth | `rust/crates/zg-codegraph/**`, `rust/crates/zg-graph-projection/**`, `rust/Cargo.lock`, `rust/rust-toolchain.toml` |

## Fixtures

| Fixture | Role | Retrieval truth | Graph truth |
|---|---|---|---|
| `fixtures/go-workflow-discovery-v1` | tuning | qrels.json (adjudicated) | `graph-truth.json` — 18 RTA edges |
| `fixtures/go-inventory-audit-v1` | holdout | qrels.json (adjudicated) | `graph-truth.json` — 17 RTA edges |

Each fixture directory holds the frozen Go source, `manifest.json` (stable
unit IDs), `qrels.json` (adjudicated retrieval judgments),
`truth.json` (the retrieval query suite read by `run_qeval.py`), and
`graph-truth.json` (the compiler truth read by `adjudicate_graph.py` —
schema `go-graph-truth-v1`, produced by `tools/go-graph-truth`). The two
truth files are deliberately distinct, non-colliding names: they are
different files with different hashes (e.g. holdout query suite
`360bb826…` vs compiler truth `f32935fa…`), and only `*.go` source files
are copied into the work roots, so neither lane can clobber the other's
input.
`baselines.json` pins the sha256 of each truth-bearing file;
`check_fixture_hashes.py` fails the lane if any of them changes.

`tools/go-graph-truth` is the Go tool (go/packages + SSA + RTA) that
regenerates the compiler truth. It is committed for regeneration only — the
truth JSON is committed, so **the graph lane needs no Go toolchain at CI
time**; it only adjudicates the produced codegraph against static truth.

## The contract: exact equality, no retries

`baselines.json` is the machine-checked acceptance reference. Every
measurement to date has been byte-reproducible across machines, so the lanes
compare with **exact equality** and use **no retries** — determinism is the
contract, a flake is a finding. Any deviation fails the lane:

- **Retrieval lane** (`run_qeval.py`): copies a fixture's sources into a
  fresh work root, builds the index once (`--refresh wait` warmup), runs the
  measured pass with `--refresh off`, normalizes compact CLI output to
  manifest unit IDs, and scores nDCG/MRR/Recall/Precision@10 against qrels
  via the vendored `evaluate_semantic_search.py`. The upstream runner has no
  in-runner repetition concept, so CI runs **two independent runner
  invocations per fixture** (separate work roots and outputs), checks BOTH
  against `baselines.json`, and byte-compares the two `normalized-runs.json`
  artifacts — 2 fixtures × 2 reps × (1 warmup + 8 queries) = 36 zg
  invocations. The model cache is warmed by the rust CLI itself (the warmup
  query passes `--model-cache`), not by any npm/node path.
- **Graph lane** (`adjudicate_graph.py`): runs `zg --graph` on a fresh copy
  of the fixture, normalizes the `calls` edges to (fixture-relative file,
  symbol) endpoints, and adjudicates every edge against the committed
  compiler truth (endpoint identity = file + manifest symbol; bare names
  resolved per-file by exact match, else a unique `Receiver.name` suffix).
  Verdict counts, direction flips, recall, and the missed-pair list must all
  match the baseline exactly.

**Improvements fail the lane too.** A better number is still a behavior
change that was not reviewed against the qrels/truth. Improvements and
acknowledged regressions are re-baselined by a reviewed PR that edits
`baselines.json` (and, if fixtures change, their sha256 references) in the
same commit as the causing change.

## Running locally

```bash
cargo build --release -p zg          # from rust/
LANES=benchmarks/quality-lanes
ZG=rust/target/release/zg

# fixture integrity
python3 $LANES/check_fixture_hashes.py

# retrieval lane (needs the local potion model cache)
python3 $LANES/run_qeval.py --fixture $LANES/fixtures/go-workflow-discovery-v1 \
    --binary $ZG --work-root /tmp/ql-tuning --output /tmp/ql-tuning-out
python3 $LANES/check_baselines.py --baselines $LANES/baselines.json \
    --lane retrieval --fixture go-workflow-discovery-v1 --results /tmp/ql-tuning-out/metrics-k10.json

# graph lane (no model needed)
python3 $LANES/adjudicate_graph.py --fixture $LANES/fixtures/go-workflow-discovery-v1 \
    --binary $ZG --work-root /tmp/ql-graph --output /tmp/ql-graph-out.json
python3 $LANES/check_baselines.py --baselines $LANES/baselines.json \
    --lane graph --fixture go-workflow-discovery-v1 --results /tmp/ql-graph-out.json
```

Repeat with `go-inventory-audit-v1` for the holdout. `run_qeval.py` reads the
model cache from `--model-cache` or `ZVEC_GREP_MODEL_CACHE` (default
`~/.engram/zvec-grep/model`); CI caches the `local/potion-code-16m-v2`
download like the existing local-model smoke jobs.

## Provenance and scope

### Vendor deltas (run_qeval.py vs upstream-sync-t1)

The runner is vendored from the engram upstream-sync Tier-1 runner with these
documented deltas (everything else — warmup semantics, zg invocation flags,
normalization, scoring — is unchanged):

1. `sys.path` points at this directory (repo-relative) instead of hardcoding
   the engram checkout; imports come from the vendored
   `evaluate_semantic_search.py` / `replay_normalization.py`.
2. Added `--model-cache` (default: `ZVEC_GREP_MODEL_CACHE`, else
   `~/.engram/zvec-grep/model`) so CI can point at the cached potion model —
   the upstream hardcodes the home-directory constant.
3. Dropped the unused `--label` argument and the stale `--warmup` docstring
   mention (argparse never defined it upstream).

### Holdout fixture lock and baseline reconciliation

`go-inventory-audit-v1` was **user-locked on 2026-11-16** after adversarial
review (fixes applied and re-verified); its qrels/manifest/compiler-truth
hashes are pinned in `baselines.json`. The vendored `REPORT.md` originally
carried a stale "DRAFT (not locked)" title, corrected in the vendored copy.

The holdout REPORT.md records an **older** baseline
(`0.5946 / 0.7875 / 0.725 / 0.275`) measured on engine `214ba02` with a
different harness driver (`run-lane.mjs`) before the promotion/fusion retune.
The lanes pin the **Step 0** post-retune numbers
(`0.6308114991329224 / 0.78125 / 0.7833333333333333 / 0.3`), measured with
this vendored `run_qeval.py` harness on locked fixtures — that is the lane
reference; the REPORT figure is historical context only.

### Other vendored pieces

- `evaluate_semantic_search.py` and the normalization subset in
  `replay_normalization.py` are vendored from
  `jordigilh/engram` (`scripts/evaluate_semantic_search.py`,
  `scripts/replay_synthetic_semantic_search.py`, 2026-11-18) so this fork is
  self-contained and deliberately diverged. Re-vendor by reviewed copy.
- These lanes are the **PR gate** with exact pinned baselines. They are
  distinct from `.github/workflows/retrieval-only.yml`, the existing
  maintainer-gated manual benchmark harness (workflow_dispatch, candidate
  refs, SWE-QA/BEIR/Quarry suites) — do not conflate the two.
- The graph lane covers the **native** zg codegraph (`zg --graph`).
  Projection-reader adjudication stays manual: it requires an
  upstream-built store and is documented in the projection-reader spike
  evidence (see the PR that added these lanes).
- The holdout fixture was rescued from a temp directory after an
  environment reboot and hash-verified on commit; its original build report
  is `fixtures/go-inventory-audit-v1/REPORT.md`.
