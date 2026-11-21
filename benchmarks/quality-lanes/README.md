# Quality lanes

Retrieval-quality and graph-quality regression gates for the fork. Implements
[jordigilh/zvec-grep#49](https://github.com/jordigilh/zvec-grep/issues/49).

Three lanes run on every PR whose diff touches their trigger paths (see
[`.github/workflows/quality-lanes.yml`](../../.github/workflows/quality-lanes.yml)):

| Lane | Gates | Trigger paths (in addition to `benchmarks/quality-lanes/**`) |
|---|---|---|
| `retrieval-quality` | hybrid semantic-search retrieval metrics vs adjudicated qrels (Go, exact) | `rust/crates/zg-engine/**`, `src/engine/**`, `src/index.ts`, `src/index-progress.ts`, `rust/Cargo.lock`, `rust/rust-toolchain.toml` |
| `retrieval-quality-multilanguage` | same metrics for Go, Python, Rust and TypeScript, equal-or-better per language and metric vs `baselines-multilanguage.json` | same as `retrieval-quality` |
| `graph-quality` | native codegraph call-edge quality vs compiler truth | `rust/crates/zg-codegraph/**`, `rust/crates/zg-graph-projection/**`, `rust/Cargo.lock`, `rust/rust-toolchain.toml` |

Beyond PR gating, the workflow also runs on **pushes to `main`** — this
mechanizes the standing belt-and-suspenders rule of byte-identically
confirming the final merged state, not just the proposed diff — and on
**`workflow_dispatch`**, a manual lever for maintainers to re-verify any
checkout on demand. Both are deliberate deviations from the original plan's
"PR-triggered only" scope, kept because they cannot weaken the gate.

## Fixtures

| Fixture | Role | Retrieval truth | Graph truth |
|---|---|---|---|
| `fixtures/go-workflow-discovery-v1` | tuning | qrels.json (adjudicated) | `graph-truth.json` — 18 RTA edges |
| `fixtures/go-inventory-audit-v1` | holdout | qrels.json (adjudicated) | `graph-truth.json` — 17 RTA edges |
| `fixtures/python-workflow-discovery-v1` | multi-language (no regression) | qrels.json (adjudicated) | — |
| `fixtures/rust-workflow-discovery-v1` | multi-language (no regression) | qrels.json (adjudicated) | — |
| `fixtures/typescript-workflow-discovery-v1` | multi-language (no regression) | qrels.json (adjudicated) | — |
| `fixtures/java-workflow-discovery-v1` | multi-language (no regression) | qrels.json (adjudicated) | — |

## Multi-language lane (no regression)

`run_qeval_multilanguage.py` is the four-language variant of `run_qeval.py`.
It reads the non-compact `--preview full` output and maps each hit from its
`symbol: <kind> <name> [scope: <Scope>]` line to the manifest unit ID
(`<path>::<name>` or `<path>::<Scope>.<name>`), falling back to manifest source
spans for hits that carry no resolvable symbol (class fields, module
declarations, file-level chunks). Unmatched hits are counted and reported,
never silently dropped. One warmup index build and two frozen-index repeats per
fixture; a ranking that differs between repeats aborts the run.

`baselines-multilanguage.json` pins one floor per language and metric.
`check_baselines_multilanguage.py` enforces **equal or better** for every
metric: a regression fails, an improvement passes and is printed (re-pin it in
a reviewed change so the floor cannot silently rot). There is no cross-language
average and no quality tolerance; only float representation noise (1e-12) is
allowed. The pinned values were measured on macOS/arm64 and the same numbers
were reproduced byte-for-byte on the `ubuntu-24.04` CI runner before the label
corrections below, so the floor is not host-specific.

The Rust and TypeScript truth/qrels were corrected in Engram commit
`9032f43` (direct membership-rejection path graded consistently with
`interactive-selection-guard`; unsupported parameter-validation positives
demoted; marker-synthesis rationales aligned with the source) and re-vendored
here. Source files and snapshot digests are unchanged.

The Python lane carried the same defects and was corrected in `7752ae9`.
The Java lane was adjudicated in Engram `ca6231c` after a blind audit (labels
withheld; 88.4% threshold agreement), with six boundary corrections applied.
Its measured floor is materially lower than the other four lanes (nDCG@10
0.325 vs 0.46-0.62). That is the current Java retrieval result, not a fixture
defect: class-level Javadoc makes container units strong lexical matches, and
the floor is intentionally kept rather than tuning labels to raise it. Treat
the Java floor as its own regression baseline and never as a cross-language
comparison.

Each gated Go fixture directory holds the frozen Go source, `manifest.json`
(stable unit IDs), `qrels.json` (adjudicated retrieval judgments),
`truth.json` (the retrieval query suite read by `run_qeval.py`), and
`graph-truth.json` (the compiler truth read by `adjudicate_graph.py` —
schema `go-graph-truth-v1`, produced by `tools/go-graph-truth`). The two
truth files are deliberately distinct, non-colliding names: they are
different files with different hashes (e.g. holdout query suite
`360bb826…` vs compiler truth `f32935fa…`), and the Go lanes copy only
`*.go` source files into their work roots, so neither lane can clobber the
other's input. The multi-language runner copies the sources selected by each
fixture's own `manifest.source` include/exclude list.
`baselines.json` pins the sha256 of **every file in this tree** — fixture
sources, query suites, compiler truth, all scripts, READMEs, and the
regeneration tool — except `baselines.json` itself (it holds the pins).
`check_fixture_hashes.py` fails the lane if any pinned file changes, and
also if a file appears without a pin; re-pinning happens only through
`check_fixture_hashes.py --regenerate` as part of a reviewed change.

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
  Verdict counts, direction flips, recall, the missed-pair list, and the
  **full verdict sets** must all match the baseline exactly: the comparator
  pins the exact confirmed-pair set, the exact refuted-pair set, and the
  exact pending-external list (source + unresolved target name), so a change
  in the composition of any verdict class fails even when aggregate counts
  are unchanged.

**Improvements fail the lane too.** A better number is still a behavior
change that was not reviewed against the qrels/truth. Improvements and
acknowledged regressions are re-baselined by a reviewed PR that edits
`baselines.json` (and, if fixtures change, their sha256 references) in the
same commit as the causing change.

## Running locally

Use **CPython 3.12**, the version CI uses. Earlier versions can sum
floating-point scores differently (the `sum()` of floats changed in 3.12)
and fail the exact baseline check. Run every command from the repository
root:

```bash
cargo build --manifest-path rust/Cargo.toml --release --locked -p zg
LANES=benchmarks/quality-lanes
ZG=rust/target/release/zg

# fixture integrity
python3 $LANES/check_fixture_hashes.py

# fresh temp parent per invocation, so reruns cannot reuse stale metrics
TMP=$(mktemp -d)

# retrieval lane (needs the local potion model cache)
python3 $LANES/run_qeval.py --fixture $LANES/fixtures/go-workflow-discovery-v1 \
    --binary $ZG --work-root "$TMP/tuning-work" --output "$TMP/tuning-out" \
  && python3 $LANES/check_baselines.py --baselines $LANES/baselines.json \
    --lane retrieval --fixture go-workflow-discovery-v1 \
    --results "$TMP/tuning-out/metrics-k10.json"

# graph lane (no model needed; requires the Python `zstandard` package
# or the `zstd` executable to decompress the codegraph artifact)
python3 $LANES/adjudicate_graph.py --fixture $LANES/fixtures/go-workflow-discovery-v1 \
    --binary $ZG --work-root "$TMP/graph-work" --output "$TMP/graph-out.json" \
  && python3 $LANES/check_baselines.py --baselines $LANES/baselines.json \
    --lane graph --fixture go-workflow-discovery-v1 --results "$TMP/graph-out.json"
```

Run each checker only after its producer succeeds (`&&` above). Repeat
with `go-inventory-audit-v1` for the holdout, using a fresh `TMP` parent. `run_qeval.py` reads the
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
