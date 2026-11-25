# Context-cap A/B harness

Measures the retrieval impact of changing a transformer model's
`max_input_tokens` (the context cap) on frozen real-world corpora, using
ranking agreement plus manual inspection of divergences. Built for issue
jordigilh/zvec-grep#73 (8192 → 1024 for `jina-embeddings-v2-base-code` and
`gte-modernbert-base`); designed to be reused when the GPU backend (#74)
makes larger caps affordable again.

## Why ranking agreement, not graded metrics

Adjudicated qrels exist only for the `quality-lanes` fixtures, which are too
small to produce fragments near the cap. Real corpora have no ground truth,
so this harness measures:

1. **Top-10 overlap** between the old-cap and new-cap index, on `path:span`
   units and on files. Span overlap is mechanically deflated when the cap
   changes (chunk boundaries move), so file overlap is the stabler signal.
2. **Manual inspection** of the largest divergences — the only quality
   direction evidence. Overlap alone cannot distinguish "different" from
   "worse".
3. **An invariance control**: a pair using a model the cap change does not
   touch (qwen3 / llama-cpp) must return identical rankings — ordered-equal
   top-10 lists, not just overlapping sets (`ordered_equal` in
   `compare-pair.py` output). If it does not, something other than the cap
   moved.

## Method

For each corpus, one A/B pair:

1. Copy the corpus twice (`pair.sh`), index side A with the baseline binary
   and side B with the experimental binary, same explicit model, direct mode,
   CPU, fresh `ZVEC_GREP_HOME` per side.
2. Query both sides with **each side's own binary** (`compare-pair.py`).
   This is mandatory: the index manifest records `maxInputTokens`, the engine
   treats a cap change as a model-identity change, and the new binary refuses
   to query an old-cap index ("existing index uses a different embedding
   model; rebuild the index"). Query texts are short, so query embeddings are
   identical under either cap and the comparison stays read-only.
3. Inspect the per-query JSON dumps for the largest divergences and judge
   whether the new side is better, equal, or worse.

### Traps (all hit at least once on 2026-11-24)

- **Per-side binaries**: see step 2. Querying both sides with one binary
  either fails (new binary, old index) or silently compares against a
  rebuilt index.
- **Anchored rsync excludes**: `--exclude analysis` dropped the Java corpus's
  `com/projectlightpath/analysis/` source package (58-file corpus instead of
  ~190). Always anchor: `--exclude /analysis`.
- **Workspace registry collisions**: `~/.zvec-grep/workspaces.json` is global
  regardless of `ZVEC_GREP_HOME`; re-running a pair whose work-root *basename*
  was registered before fails with "Workspace name ... is already registered".
  Remove the stale entries (or use fresh pair names) before re-running.
- **CPU contention**: index both sides sequentially and run nothing else;
  wall-time comparisons are meaningless otherwise.

## Frozen sample: 2026-11-24

`~/.engram/zvec-grep/context-ab/corpora-2026-11-24.tar.gz`
(sha256 `a903828a8428743835e4e5bc95bbfcc34e45195147aa3bf52f0d80e424aed718`,
~417 MB) holds the exact indexed trees (`.zvec-grep` excluded). Upstream
revisions at capture time, several with uncommitted work — the tarball, not
the clone, is the authoritative sample:

| Corpus | Model | Upstream revision | Notes |
|---|---|---|---|
| kuadrant-operator (Go, 332 .go source files, 826 indexed total) | gte | `21ed74e2` (v0.1.0-2645-g21ed74e2, dirty) | full repo incl. generated CRD/bundle YAML |
| kubernaut-console (TS) | gte | `23a3f24a` (v1.6.0-rc28-55-g23a3f24, dirty) | node_modules/dist/storybook excluded |
| java-analysis-adapter (Java, 227 files) | qwen3 (control) | `f17f31a` (clean) | src only; anchored excludes `/spike`, `/analysis`, `/target` |
| praxis-grid (Rust) | jina | `3299794` (v0.1.3-57-g3299794, dirty) | target excluded |

Query suites: `queries-{go,ts,java,rust}.json` (natural-language questions
written from each repo's structure before running; do not tune them to the
results).

## Results: 8192 → 1024 (issue #73)

| Pair | Index wall A→B | Top-10 overlap span / file | Inspection |
|---|---|---|---|
| go / gte | —→1720s | 6.8 / 5.7 | mixed; one clearly better (B) |
| ts / gte | 1418s→698s | 6.1 / 5.1 | focused but flooded (B) |
| java / qwen3 (control) | 2224s→2448s | **10.0 / 10.0** | invariance confirmed on the full 227-file corpus |
| rust / jina | 865s→496s | 7.7 / 5.7 | clearly better (B) |

The Java pair ran twice: the first copy accidentally excluded the
`com/projectlightpath/analysis/` source package (58 files); the numbers above
are the full 227-file corpus. Its near-equal wall times are expected — the
llama-cpp path is identical in both binaries — and the 10/10 control is the
point of that pair, not its speed.

Observed mechanism, two effects in opposite directions:

1. **Dilution → specificity.** At 8192 a giant generated file (CRD/bundle
   YAML) is one mean-pooled chunk whose blurry vector matches broadly and
   outranks precise source-file matches. At 1024 the same file splits into
   sharp chunks; the region containing the queried content ranks on merit.
   This is why the 1024 side promoted reconciler sources over generated YAML
   (go q18) and Makefile/xtask sources over docs (rust q09).
2. **Chunk flooding.** At 1024 a giant file contributes many chunks, and
   several can crowd out top-10 diversity — ts q08 had 7 of 10 hits from one
   huge mockup HTML file.

So: **8192 = whole-file relevance without locality; 1024 = locality with
flooding risk on giant files.** The optimum is plausibly between them and
query-dependent (a 3000-line mockup wants a different cap than a 40-line
Go function).

## Rerun plan: 1024 / 2048 / 4096 / 8192 once GPU lands (#74)

The point of the frozen sample. When the MLX backend makes larger caps
affordable, run the cap as the only variable:

1. Build four binaries that differ **only** in the catalog
   `max_input_tokens` for the model under test (1024, 2048, 4096, 8192) —
   one commit per cap or a build-time override, nothing else changed.
2. For each corpus × cap: `pair.sh` with the 1024 binary as side A
   (the landed baseline) and the cap binary as side B, same model, same
   queries. Keep the qwen3 control pair to prove binary invariance.
3. Record per cap: top-10 overlap (span and file) vs the 1024 baseline,
   index wall time, peak RSS, and **hits-per-file in top-10** (the chunk
   flooding metric — add it to `compare-pair.py` output).
4. Inspect the same worst divergences at each cap; the interesting
   comparisons are 2048 vs 1024 (does flooding shrink without losing the
   specificity win?) and 4096 vs 8192 (does dilution begin, or is 4096
   strictly better?).
5. Decision rule: a cap raise is justified only if inspected quality is
   not worse AND the flooding metric does not regress AND the graded
   `quality-lanes` floors still pass at the new cap (they must be re-run;
   chunk boundaries change, so floors may need a reviewed re-pin).

### Spike arms (added 2026-11-24): adaptive chunking vs query-time merging

Two additional arms test cheaper alternatives to a blanket cap raise.
**Dynamic per-file caps (cap = file size, up to 8192) are excluded by
design**: whole-file embeddings of large files are the dilution behavior
measured as worse in the 8192 run, one 8192-token sequence costs ~3.2 GB
of attention scores (the OOM class PR #77 removed), and `max_input_tokens`
is part of model identity, so per-file variability needs a new index
identity scheme.

- **Arm C — adaptive chunk target (2048).** Chunk budget
  `min(file tokens, 2048)` instead of a flat 1024: medium files stay one
  chunk, giant files still split. Implement as a plain 2048 catalog cap
  (arm 2 above), which is exactly this behavior — no new code. Measures
  whether the flooding shrink from 2048 outweighs any dilution loss.
- **Arm M — query-time chunk merging.** Keep the flat 1024 cap (sharp
  chunk vectors, unchanged model identity) and merge adjacent chunks of
  the same file into one hit at ranking time, presenting whole-file
  presence plus a local pointer. A ranking-layer change only; needs a
  `zg-engine` merge step plus a `compare-pair.py` mode that scores merged
  hits. Measures locality + diversity without touching embeddings.

Run M against the 1024 baseline (same binary pair as the #73 A/B, plus
the merging change on side B) and against arm C. The flooding metric
decides: if M reduces hits-per-file at equal or better inspected quality,
it dominates every cap raise for the flooding problem, and the cap
question reduces to dilution alone (arms 2–4).

Hypothesis to falsify: **2048 keeps most of the specificity win while
halving the flooding effect; beyond 4096 dilution dominates and retrieval
on generated-artifact-heavy repos degrades back toward the 8192 behavior.
Query-time merging (M) matches or beats every static cap on the flooding
metric at 1024 cost.**

## Files

- `pair.sh` — build one A/B pair (env: `ZG_A_BIN`, `ZG_B_BIN`, `ZG_AB_BASE`)
- `compare-pair.py` — per-side-binary ranking comparison (same env)
- `queries-{go,ts,java,rust}.json` — frozen query suites
- `corpora.json` — machine-readable corpus manifest for the rerun
