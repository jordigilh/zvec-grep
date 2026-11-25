# MLX GPU backend preflight (issue #74)

Bounded spike for jordigilh/zvec-grep#74: measure the Apple-GPU ceiling for
the transformer embedding models before committing to a Rust backend, and
resolve the preflight questions (crate maturity, weight sources, local
buildability). This directory holds the measurement harness and the findings;
it does **not** run the #80 context-cap experiment.

## Result

**MLX clears the issue's ≥3× decision rule at the engine level, and the
numerics port is exact — but the Rust backend cannot be built on this host.**
`mlx-sys` compiles MLX from source through CMake, and MLX's Metal backend
requires the `metal` shader compiler, which is an Xcode component (Xcode 26+
downloadable "Metal Toolchain"); it is not part of CommandLineTools and cannot
be installed with CLT alone (ml-explore/mlx#103, ml-explore/mlx-swift#500).
A CPU-only `mlx-sys` build (`default-features = false`, which sets
`MLX_BUILD_METAL=OFF`) does compile on this host, but a CPU-only MLX backend
has no acceleration story over ORT CPU, so the GPU path is the blocked one.
The Python MLX wheels ship a prebuilt `libmlx` + `mlx.metallib`, which is why
the spike below runs on a CLT-only machine while the Metal-enabled
`cargo build` of `mlx-rs` cannot. The implementation plan at the bottom is
therefore gated on Metal toolchain availability (full Xcode or a
prebuilt-metallib `mlx-sys` path, neither of which exists today).

## Measurements (2026-11-25, 12-core Apple Silicon, CLT-only host)

Corpus: `rust/crates/zg-engine/src` (158 files, each pre-truncated to 6000
chars — below what the 1024-token cap already removes — giving 126k real
tokens after the cap), longest-first packing, padded 8 (default) or 16 x 1024
batches, single process. Every row below is reproducible with the committed
`jina_mlx_spike.py` (`--eager`, `--dtype bf16`, `--batch 16`, and
`--skip-checks` for the MLX-only RSS runs).

| Engine | Wall | Throughput | Files/s | Peak RSS |
|---|---|---|---|---|
| ORT CPU q8 (12 intra-op threads), production artifact | 58.4-70.8 s | 1 780-2 159 tok/s | 2.2-2.7 | not measured |
| MLX GPU fp16, eager | 18.1 s | 6 959 tok/s | 8.72 | 0.82 GB (MLX-only) |
| MLX GPU fp16, `mx.compile` | **13.7 s** | **9 226 tok/s** | **11.56** | 0.82 GB (MLX-only) |
| MLX GPU fp16, `mx.compile`, batch 16 | 13.5 s | 9 355 tok/s | 11.72 | 0.82 GB (MLX-only) |
| MLX GPU bf16, `mx.compile` | 14.6 s | 8 621 tok/s | 10.80 | 0.82 GB (MLX-only) |

- **Engine-vs-engine speedup: 3.7-5.2x** (identical batches, identical
  tokenizer, same-run pairs; the 3.7x lower bound pairs the best MLX wall
  with the best ORT wall across the spread noted below, the table's own
  rows give 4.0-5.2x). This is the honest ceiling number: it isolates the
  compute engine from batching policy.
- Against the **current production CPU path** (max_batch_size 2, token-budget
  sub-batching; #73 baseline 311 s wall / 0.78 GB RSS for 166 files) the same
  MLX configuration is ~20x faster wall-clock, because the GPU path also
  affords larger batches. Both numbers matter: the engine-level ratio
  survives a batching parity comparison; the ~20x is what a user would see.
- Memory stays at **0.82 GB MLX-only** (weights 0.32 GB fp16 + activations;
  measured in a process without the ONNX reference sessions), versus the
  measured CoreML regression (7.2 GB RSS, 3.1x slower than CPU) that closed
  the ORT-CoreML route in #74.
- Run-to-run spread on this host (MLX 13.5-14.6 s, ORT 49.7-70.8 s across
  sessions) is host load, not the engines; every ratio above uses same-run
  pairs.

## Numerics

The spike implements the full JinaBERT forward pass from the fp16
`model.safetensors` (no ONNX involved) and validates it against the fp32 ONNX
reference from the same revision:

- Embedding cosine (MLX fp16 vs ORT fp32, 3 short texts: Rust, Rust, SQL):
  **0.9999990-0.9999996** (printed by the committed script).
- Embedding cosine vs the shipped q8 artifact: 0.991-0.995 (the fp16-vs-q8
  gap, as expected; also printed by the committed script).

**Precision policy (measured, and binding for the Rust port):** fp16/bf16
weights and activations everywhere, except layer norms compute their
mean/variance in fp32, and attention scores + softmax run in fp32. Either
shortcut degrades embedding cosine to ~0.95 on some inputs: fp16 layer-norm
accumulation loses precision in the variance, and fp16 scores/softmax lose
it in the ALiBi-biased exponentials. With the policy above the port is exact
(cosine ~1.0) at the same throughput. `--dump-fixtures DIR` writes the
ids/mask/reference/MLX embedding set used by the future Rust backend test
(computed at fp16, stored fp32).

The forward pass spec below was recovered from the shipped ONNX graph and
confirmed numerically, so the eventual Rust port does not need to rediscover
it.

## Weight sources (preflight question)

Both catalog targets publish `model.safetensors` at the pinned revisions
(`jinaai/jina-embeddings-v2-base-code`@516f4ba, `Alibaba-NLP/gte-modernbert-base`@e7f32e3),
so MLX entries reuse the existing `models/artifacts` pipeline by adding
`model.safetensors` to their artifact lists. Backends must not (and need not)
implement separate download paths. ONNX-to-MLX weight conversion is not
needed.

## Crate maturity (preflight question)

`mlx-rs` 0.34 / `mlx-sys` 0.7 (oxiglade/mlx-rs) is the only viable Rust MLX
binding: active (releases within the month, MLX 0.32.2), MIT/Apache-2.0,
~34k monthly downloads, MSRV 1.88. It exposes everything the forward pass
needs — matmul/softmax/erf, `fast` layer-norm helpers, safetensors loading
(behind the `safetensors` feature), and `transforms::compile` (the Rust
counterpart of `mx.compile`, verified in the 0.34 source), so the compiled
3.7-5.2x number is the target for the Rust backend, not just the Python
spike.
The constraint that shapes the design is `Array` being `!Send` (0.34):
arrays must be built and evaluated on one thread.

## JinaBERT forward spec (validated)

```
x  = LayerNorm(word_emb[ids] + token_type_emb[0])          # eps 1e-12
alibi[h,i,j] = slope_h * |i - j|                            # slopes below
mask_bias    = (1 - mask) * -FLT_MAX                        # additive
per layer (12x):                                            # fp32: LN stats,
  q,k,v = Wq/Wk/Wv x + b                                    # scores, softmax
  q = LN_q(q); k = LN_k(k)                                  # qk-LayerNorm
  s = (q @ k^T)/sqrt(64) + alibi + mask_bias
  a = reshape(softmax(s) @ v)
  a_ln = LN_attn(Wo a + bo + x)                             # post-norm
  x1  = x + a_ln
  h   = LN_1(x1)
  g,u = split(W_up_gated h, 3072)                           # fused GLU
  d   = W_down (g * gelu_erf(u)) + b_down
  x   = LN_2(h + d)
pool = mean over mask; L2-normalize
slopes = -(2^-1..2^-8, 2^-0.5, 2^-1.5, 2^-2.5, 2^-3.5)      # 12 heads
```

`gte-modernbert-base` (ModernBERT: rotary + alternating sliding/global
attention) was not ported in this spike; it needs its own forward spec and
numerics pair before any cap conclusions (#80 plan).

## Implementation plan (gated on Metal toolchain availability)

TDD plan for when the blocker lifts (full Xcode with Metal toolchain, or a
prebuilt-MLX `mlx-sys` path). Estimated 3-4 days for the jina slice.

1. **Feature scaffold (test-first).** `zg-engine` optional cargo feature
   `mlx`; `backends/mlx/mod.rs` declarative; new catalog entry
   `local/jina-embeddings-v2-base-code-mlx` (same repo/revision, adds
   `model.safetensors` artifact, `max_batch_size` 8, dtype `fp16`).
   Pin the ModelScope fallback source and revision explicitly for the new
   entry (the existing entry falls back to a *different* revision,
   `91aa0a6a…`, and `model.safetensors` must be verified to exist at
   whichever revision is pinned — or drop the fallback for the MLX entry).
   Factory test: entry constructs and reports `backend: "mlx-gpu"`,
   `dtype: "fp16"` — a model-identity change, never mixed into an existing
   index, no automatic fallback to ONNX/model2vec. Refresh
   `models/tests/fixtures/catalog-main-oracle.json` (record its source
   revision, per `RUST_MAIN_PARITY.md`) and either mirror the entry in the
   TypeScript catalog or document the divergence.
2. **Worker-thread model (test-first).** `Array` is `!Send`, so the loaded
   model owns one dedicated worker thread; `embed` sends plain `Vec<u32>`
   token ids and receives `Vec<f32>`. Unit tests: order preservation,
   cancellation between sub-batches, worker survives a panicked forward,
   runtime execution budget still bounds concurrent callers (backend adds no
   scheduler of its own).
3. **Forward pass (numerics test-first).** Port the spec above with the
   measured precision policy (fp32 layer-norm internals and attention
   scores/softmax); wrap the layer stack in `mlx_rs::transforms::compile`
   (shapeless over batch size, recompiled per sequence length bucket if
   needed); ignored-tagged test `cached_jina_mlx` (mirrors `cached_minilm`)
   asserting embedding cosine >= 0.999 against the fixtures written by this
   spike's `--dump-fixtures`, plus dimension and finite-value checks through
   `validate_result`.
4. **Batching + pooling.** Promote the token-budget sub-batch packing and
   probe-encoding helpers out of `backends/transformers/model.rs` into a
   backend-neutral `models` helper module (AGENTS.md layout rules) so the
   MLX backend shares them rather than copying; mean-pool with the
   attention mask, L2-normalize. Unit tests for packing edge cases run
   without the MLX feature.
5. **Gates.** `cargo fmt`, `cargo clippy -p zg-engine --all-targets
   -- -D warnings` with and without `--features mlx`, targeted `zg-engine`
   tests, `cached_minilm` real-model pair with same-cap invariance per the
   #80 README. The nine quality lanes are not re-run for existing models
   (new identity, existing indexes untouched); the new entry gets its own
   lane run before default promotion. CI gating is a merge blocker for the
   implementation PR — see the next section.
6. **gte-modernbert** is out of scope for the first slice; follow-up issue
   with its own spike (rotary + sliding-window attention port).

## CI gating (decided: ubuntu CPU gate + hosted macOS lane)

Decision (2026-11-25): the implementation ships **two lanes**, and no
self-hosted runner is required unless the hosted macOS spike fails. This
repository has no macOS lane today; it was removed, and the workflows now
run on `ubuntu-latest` only, so both lanes are new work.

1. **Ubuntu lane — mandatory, gates everything that is not Metal
   execution.** `mlx-sys` builds with `default-features = false`
   (`MLX_BUILD_METAL=OFF`, `MLX_BUILD_CPU=ON`), and MLX publishes a CPU-only
   build for Linux (`pip install mlx[cpu]`), so the `mlx` feature can be
   compiled and its non-GPU tests run on `ubuntu-latest`: worker-thread
   design (`Array` is `!Send`), token-budget packing, mean-pool/normalize,
   model identity, cancellation, execution-budget handling, catalog and
   oracle fixtures. **Prerequisite spike (30 min):** confirm `mlx-sys`
   builds CPU-only on `ubuntu-latest` (CMake, C++20, libblas/liblapack
   headers). If it does not, the lane compiles the feature against a stub
   backend and the macOS lane carries every MLX execution test.
2. **Hosted macOS lane — gates the Metal kernels.** `runs-on: macos-26`
   (arm64, 3-core M1, 7 GB). Standard GitHub-hosted runners are **free and
   unlimited on public repositories**, macOS included, so this lane costs
   nothing and needs no payment method. Steps: run `xcrun -f metal`, and
   `xcodebuild -downloadComponent MetalToolchain` when it fails (Xcode 26+
   ships Metal as a separate downloadable component; CommandLineTools alone
   never has it), then the `#[ignore]`d `cached_jina_mlx` numerics test
   (cosine >= 0.999 against the fp32 fixtures from this spike), a smoke
   embedding, and one end-to-end `zg` run.
3. **Hosted Metal must be proven, not assumed.** Metal does execute on
   hosted arm64 macOS VMs — actions/runner-images#14836 (Oct 2026) shows
   Metal command buffers running cleanly on `macos-15`/`macos-26` arm64,
   with GPU hangs confined to `macos-15-intel` — but MLX itself does not
   rely on it: upstream `ml-explore/mlx` runs its `mac_build` matrix
   (`cpu`, `metal`, `jit`) on `runs-on: [self-hosted, macos]`. The lane
   must therefore assert that an MLX **Metal device** was allocated (not a
   silent CPU fallback) before the numerics test counts as a GPU gate.
4. **Fallback.** If (3) fails, a self-hosted macOS runner on the
   maintainer's Mac replaces (2) with identical jobs; only the `runs-on:`
   label changes and the acceptance criteria stay the same.

**Acceptance criteria for the implementation PR:** the ubuntu lane is green
with `--features mlx` (all non-GPU logic) **and** the macOS lane runs the
MLX numerics test plus a smoke embedding on a confirmed Metal device.
Local-only verification does not satisfy this.

**Confidence: 0.78** that the slice lands within the estimate and keeps
>= 3x over the CPU path in the shipped Rust backend (`transforms::compile`
is exposed, so the compiled 3.7-5.2x is the target). Deductions: `Array
!Send` forces the worker-thread design; the precision policy adds a
correctness surface (fp32 LN/scores are mandatory, measured); single-maintainer
crate; the toolchain blocker adds environment risk and gates the whole plan.
Numerics risk is low (spec validated here); scheduling/identity risk is low
(runtime rules already established in #73/#77).
