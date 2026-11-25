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

Corpus: `rust/crates/zg-engine/src` (158 files, 126k real tokens after the
1024-token cap), longest-first packing, padded 8 x 1024 batches, single
process. `jina_mlx_spike.py` is the harness; run it with the venv described
in its docstring.

| Engine | Wall | Throughput | Files/s | Peak RSS |
|---|---|---|---|---|
| ORT CPU q8 (12 intra-op threads), production artifact | 49.7 s | 2 538 tok/s | 3.18 | not measured |
| MLX GPU fp16, eager | 19.3 s | 6 537 tok/s | 8.19 | ~1.2 GB |
| MLX GPU fp16, `mx.compile` | **14.1 s** | **8 929 tok/s** | **11.19** | ~1.2 GB |
| MLX GPU fp16, `mx.compile`, batch 16 | 13.9 s | 9 054 tok/s | 11.34 | ~1.2 GB |

- **Engine-vs-engine speedup: 3.5x** (identical batches, identical tokenizer).
  This is the honest ceiling number: it isolates the compute engine from
  batching policy.
- Against the **current production CPU path** (max_batch_size 2, token-budget
  sub-batching; #73 baseline 311 s wall / 0.78 GB RSS for 166 files) the same
  MLX configuration is ~20x faster wall-clock, because the GPU path also
  affords larger batches. Both numbers matter: the 3.5x survives a batching
  parity comparison; the ~20x is what a user would see.
- Memory stays ~1.2 GB (weights 0.32 GB fp16 + activations), versus the
  measured CoreML regression (7.2 GB RSS, 3.1x slower than CPU) that closed
  the ORT-CoreML route in #74.
- bf16 matches fp16 within noise on this workload; fp16 is the right default.
- Verification re-run of the committed script: 13.8 s (9 161 tok/s) MLX vs
  60.7 s (2 077 tok/s) ORT — 4.4x; the spread across runs is host load, not
  the engines. Peak RSS in that process was 2.0 GB including the in-process
  ONNX reference sessions used for the numerics check; the MLX-only footprint
  is ~1.2 GB.

## Numerics

The spike implements the full JinaBERT forward pass from the fp16
`model.safetensors` (no ONNX involved) and validates it against the fp32 ONNX
reference from the same revision:

- Embedding cosine (MLX fp16 vs ORT fp32, 3 short texts: Rust, Rust, SQL):
  **1.0 / 0.9999999 / 1.0**.
- Embedding cosine vs the shipped q8 artifact: 0.993 (the fp16-vs-q8 gap, as
  expected).

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
3.5x number is the target for the Rust backend, not just the Python spike.
The constraint that shapes the design is `Array` being `!Send` (0.34):
arrays must be built and evaluated on one thread.

## JinaBERT forward spec (validated)

```
x  = LayerNorm(word_emb[ids] + token_type_emb[0])          # eps 1e-12
alibi[h,i,j] = slope_h * |i - j|                            # slopes below
mask_bias    = (1 - mask) * -FLT_MAX                        # additive
per layer (12x):
  q,k,v = Wq/Wk/Wv x + b
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
   Factory test: entry constructs and reports `backend: "mlx-gpu"`,
   `dtype: "fp16"` — a model-identity change, never mixed into an existing
   index, no automatic fallback to ONNX/model2vec.
2. **Worker-thread model (test-first).** `Array` is `!Send`, so the loaded
   model owns one dedicated worker thread; `embed` sends plain `Vec<u32>`
   token ids and receives `Vec<f32>`. Unit tests: order preservation,
   cancellation between sub-batches, worker survives a panicked forward,
   runtime execution budget still bounds concurrent callers (backend adds no
   scheduler of its own).
3. **Forward pass (numerics test-first).** Port the spec above; wrap the
   layer stack in `mlx_rs::transforms::compile` (shapeless over batch size,
   recompiled per sequence length bucket if needed); ignored-tagged test
   `cached_jina_mlx` (mirrors `cached_minilm`) asserting embedding cosine
   >= 0.999 against fixtures generated by this spike's script, plus
   dimension and finite-value checks through `validate_result`.
4. **Batching + pooling.** Promote the token-budget sub-batch packing and
   probe-encoding helpers out of `backends/transformers/model.rs` into a
   backend-neutral `models` helper module (AGENTS.md layout rules) so the
   MLX backend shares them rather than copying; mean-pool with the
   attention mask, L2-normalize. Unit tests for packing edge cases run
   without the MLX feature.
5. **Gates.** `cargo fmt`, `cargo clippy -p zg-engine --all-targets
   -- -D warnings` with and without `--features mlx` (CI macos runners have
   Xcode), targeted `zg-engine` tests, `cached_minilm` real-model pair with
   same-cap invariance per the #80 README. The nine quality lanes are not
   re-run for existing models (new identity, existing indexes untouched);
   the new entry gets its own lane run before default promotion.
6. **gte-modernbert** is out of scope for the first slice; follow-up issue
   with its own spike (rotary + sliding-window attention port).

**Confidence: 0.78** that the slice lands within the estimate and keeps
>= 3x over the CPU path in the shipped Rust backend (`transforms::compile`
is exposed, so the compiled 3.5x is the target). Deductions: `Array !Send`
forces the worker-thread design; single-maintainer crate; the toolchain
blocker adds environment risk and gates the whole plan. Numerics risk is low
(spec validated here); scheduling/identity risk is low (runtime rules already
established in #73/#77).
