//! `JinaBERT` forward pass on MLX (issue #74).
//!
//! Ported from `benchmarks/mlx-gpu/jina_mlx_spike.py`, which validates it
//! against the fp32 ONNX reference at cosine >= 0.999. The binding precision
//! policy: weights and activations are fp16, while layer-norm mean/variance and
//! attention scores/softmax run in fp32. Shortcutting either degrades cosine to
//! ~0.95 on some inputs.
//!
//! The 12-layer stack is wrapped in `transforms::compile(_, true)`. Weights are
//! passed as graph inputs (not captured) as the transform documentation
//! requires; a fresh compiled graph is installed when the batch or sequence
//! length changes, so shape-dependent reshapes are never reused across shapes.

use std::path::Path;

use half::f16;
use mlx_rs::{
    Array,
    error::Exception,
    ops,
    transforms::{compile::compile, eval},
};

use super::{
    weights::{
        EMBEDDING_INPUTS, HEAD_COUNT, HEAD_SIZE, HIDDEN_SIZE, JinaLayerWeights, JinaWeights,
        LAYER_COUNT,
    },
    worker::{MlxForward, TokenBatch},
};
use crate::models::spi::ModelError;

const EPSILON: f32 = 1e-12;
/// `sqrt(head_size)` with `head_size = 64`.
const ATTENTION_SCALE: f32 = 8.0;
const MASK_BIAS: f32 = -3.402_823_5e38;
const FIRST_LAYER_INPUT: usize = EMBEDDING_INPUTS;
const WEIGHT_INPUTS: usize = EMBEDDING_INPUTS + LAYER_COUNT * JinaLayerWeights::INPUTS;

type MlxResult<T> = Result<T, Exception>;
type CompiledForward = Box<dyn for<'a> FnMut(&'a [Array]) -> Result<Vec<Array>, Exception>>;

/// How many compiled graphs to keep. Sequence lengths are bucketed by
/// `TokenBatch::pad`, so a handful of entries cover the realistic shapes and
/// every call inside a bucket reuses the graph (and MLX's internal trace cache).
const COMPILED_CACHE_ENTRIES: usize = 4;

/// Small LRU-ish cache of compiled graphs keyed by exact `(batch, sequence)`.
///
/// `transforms::compile` keeps a per-shape trace cache inside the returned
/// closure, so the closure has to outlive a single call: rebuilding it on every
/// call throws that cache away and pays a full trace each time.
struct ShapeCache<T> {
    capacity: usize,
    entries: Vec<((i32, i32), T)>,
}

impl<T> ShapeCache<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Vec::new(),
        }
    }

    fn slot(&mut self, key: (i32, i32), build: impl FnOnce() -> T) -> &mut T {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(existing, _)| *existing == key)
        {
            let entry = self.entries.remove(position);
            self.entries.push(entry);
        } else {
            if self.entries.len() >= self.capacity {
                self.entries.remove(0);
            }
            self.entries.push((key, build()));
        }
        &mut self
            .entries
            .last_mut()
            .expect("shape cache entry was just installed")
            .1
    }
}

pub(super) struct JinaMlxForward {
    weight_inputs: Vec<Array>,
    compiled: ShapeCache<CompiledForward>,
}

impl JinaMlxForward {
    pub(super) fn load(path: &Path) -> Result<Self, ModelError> {
        let weights = JinaWeights::load(path)?;
        Ok(Self {
            weight_inputs: weights.flatten(),
            compiled: ShapeCache::new(COMPILED_CACHE_ENTRIES),
        })
    }

    fn run(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
        let batch_size = i32::try_from(batch.batch_size)
            .map_err(|_| ModelError::internal("MLX batch size does not fit an i32"))?;
        let sequence_length = i32::try_from(batch.sequence_length)
            .map_err(|_| ModelError::internal("MLX sequence length does not fit an i32"))?;

        let ids = Array::from_slice(&batch.input_ids, &[batch_size, sequence_length]);
        let type_ids = Array::from_slice(&[0_u32], &[1]);
        let mut hidden = self.weight_inputs[0]
            .take_axis(&ids, 0)
            .map_err(mlx_error)?
            .add(
                &self.weight_inputs[1]
                    .take_axis(&type_ids, 0)
                    .map_err(mlx_error)?,
            )
            .map_err(mlx_error)?;
        hidden = layer_norm(&hidden, &self.weight_inputs[2], &self.weight_inputs[3])
            .map_err(mlx_error)?;

        let mask = Array::from_slice(&batch.attention_mask, &[batch_size, sequence_length]);
        let alibi = alibi(sequence_length).map_err(mlx_error)?;
        let mask_bias = mask_bias(&mask, batch_size, sequence_length).map_err(mlx_error)?;
        let mask_pool = mask
            .reshape(&[batch_size, sequence_length, 1])
            .map_err(mlx_error)?;
        let mask_sum = mask
            .sum_axis(1, Some(true))
            .map_err(mlx_error)?
            .reshape(&[batch_size, 1, 1])
            .map_err(mlx_error)?;

        let mut inputs = Vec::with_capacity(WEIGHT_INPUTS + 5);
        inputs.push(hidden);
        inputs.push(alibi);
        inputs.push(mask_bias);
        inputs.push(mask_pool);
        inputs.push(mask_sum);
        inputs.extend(self.weight_inputs.iter().cloned());
        let function = self
            .compiled
            .slot((batch_size, sequence_length), build_compiled);
        let mut outputs = function(&inputs).map_err(mlx_error)?;
        let output = outputs
            .pop()
            .ok_or_else(|| ModelError::internal("MLX forward returned no output"))?;
        eval([&output]).map_err(mlx_error)?;

        let dimension = usize::try_from(HIDDEN_SIZE)
            .map_err(|_| ModelError::internal("invalid MLX hidden size"))?;
        let data = output.as_slice::<f32>();
        if data.len() != batch.batch_size * dimension {
            return Err(ModelError::internal(format!(
                "MLX forward returned {} values for {} vectors of {dimension}",
                data.len(),
                batch.batch_size
            )));
        }
        Ok(data.chunks_exact(dimension).map(<[f32]>::to_vec).collect())
    }
}

impl MlxForward for JinaMlxForward {
    fn forward(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
        self.run(batch)
    }
}

fn build_compiled() -> CompiledForward {
    let function = move |args: &[Array]| -> MlxResult<Vec<Array>> {
        let hidden = &args[0];
        let alibi = &args[1];
        let mask_bias = &args[2];
        let mask_pool = &args[3];
        let mask_sum = &args[4];
        let weights = &args[5..];
        let shape = hidden.shape();
        let batch_size = shape[0];
        let sequence_length = shape[1];
        let hidden = transformer_layers(
            weights,
            hidden,
            alibi,
            mask_bias,
            batch_size,
            sequence_length,
        )?;
        // Mean-pool over the mask, then L2-normalize in fp32 (the spike's
        // `pooled`), matching the ONNX sentence-embedding pipeline.
        let pooled = pool_and_normalize(&hidden, mask_pool, mask_sum)?;
        Ok(vec![pooled])
    };
    Box::new(compile(function, true))
}

fn transformer_layers(
    weights: &[Array],
    hidden: &Array,
    alibi: &Array,
    mask_bias: &Array,
    batch_size: i32,
    sequence_length: i32,
) -> MlxResult<Array> {
    let mut hidden = hidden.clone();
    for layer in 0..LAYER_COUNT {
        let layer = &weights[FIRST_LAYER_INPUT + layer * JinaLayerWeights::INPUTS..];
        hidden = transformer_layer(
            layer,
            &hidden,
            alibi,
            mask_bias,
            batch_size,
            sequence_length,
        )?;
    }
    Ok(hidden)
}

fn transformer_layer(
    layer: &[Array],
    hidden: &Array,
    alibi: &Array,
    mask_bias: &Array,
    batch_size: i32,
    sequence_length: i32,
) -> MlxResult<Array> {
    let query = layer_norm(&dense(hidden, &layer[0], &layer[1])?, &layer[6], &layer[7])?;
    let key = layer_norm(&dense(hidden, &layer[2], &layer[3])?, &layer[8], &layer[9])?;
    let value = dense(hidden, &layer[4], &layer[5])?;
    let query = split_heads(&query, batch_size, sequence_length)?;
    let key = split_heads(&key, batch_size, sequence_length)?;
    let value = split_heads(&value, batch_size, sequence_length)?;

    let scores = query
        .as_type::<f32>()?
        .matmul(&key.as_type::<f32>()?.transpose_axes(&[0, 1, 3, 2])?)?
        .divide(Array::from_f32(ATTENTION_SCALE))?
        .add(alibi)?
        .add(mask_bias)?;
    let probabilities = ops::softmax_axis(&scores, -1, None)?.as_type::<f16>()?;
    let attention = probabilities
        .matmul(&value)?
        .transpose_axes(&[0, 2, 1, 3])?
        .reshape(&[batch_size, sequence_length, HIDDEN_SIZE])?;

    let projected = dense(&attention, &layer[10], &layer[11])?;
    let normalized = layer_norm(&projected.add(hidden)?, &layer[12], &layer[13])?;
    let residual = hidden.add(&normalized)?;

    let first = layer_norm(&residual, &layer[14], &layer[15])?;
    // The fused GLU projection was pre-split at load, so the compiled graph has
    // no shape-inferring slice: `gate` is rows[:3072], `up` rows[3072:].
    let gate = first.matmul(&layer[16].transpose_axes(&[1, 0])?)?;
    let up = first.matmul(&layer[17].transpose_axes(&[1, 0])?)?;
    let down = dense(&gated_activation(&gate, &up)?, &layer[18], &layer[19])?;
    layer_norm(&first.add(&down)?, &layer[20], &layer[21])
}

/// `gate * gelu(up)`, with the erf form of GELU the validated spike uses.
///
/// Kept separate from the layer so the gate/up ordering — swapping the two is
/// a silent quality regression, not a crash — is covered by a deterministic
/// test that needs neither Metal nor a downloaded model.
fn gated_activation(gate: &Array, up: &Array) -> MlxResult<Array> {
    let scaled = up.divide(Array::from_f32(std::f32::consts::SQRT_2))?;
    let activated = ops::erf(&scaled)?
        .add(Array::from_f32(1.0))?
        .multiply(up)?
        .multiply(Array::from_f32(0.5))?;
    gate.multiply(&activated)
}

/// Mean-pool over the attention mask, then L2-normalize in fp32.
///
/// The spike's `pooled`/`normalized` pair; extracted for the same reason as
/// [`gated_activation`].
fn pool_and_normalize(hidden: &Array, mask_pool: &Array, mask_sum: &Array) -> MlxResult<Array> {
    let pooled = hidden
        .multiply(mask_pool)?
        .sum_axis(1, Some(true))?
        .divide(mask_sum)?
        .as_type::<f32>()?;
    let norm = pooled.multiply(&pooled)?.sum_axis(-1, Some(true))?.sqrt()?;
    pooled.divide(&norm)
}

/// `x @ Wᵀ + b` for a `[out, in]` projection.
fn dense(input: &Array, weight: &Array, bias: &Array) -> MlxResult<Array> {
    input.matmul(&weight.transpose_axes(&[1, 0])?)?.add(bias)
}

/// `[b, n, hidden]` -> `[b, heads, n, head_size]`.
fn split_heads(input: &Array, batch_size: i32, sequence_length: i32) -> MlxResult<Array> {
    input
        .reshape(&[batch_size, sequence_length, HEAD_COUNT, HEAD_SIZE])?
        .transpose_axes(&[0, 2, 1, 3])
}

/// Layer norm with fp32 statistics, returning fp16 (the activation dtype).
fn layer_norm(input: &Array, weight: &Array, bias: &Array) -> MlxResult<Array> {
    let input = input.as_type::<f32>()?;
    let mean = input.mean_axis(-1, Some(true))?;
    let centered = input.subtract(&mean)?;
    let variance = centered.multiply(&centered)?.mean_axis(-1, Some(true))?;
    let normalized = centered.divide(&variance.add(Array::from_f32(EPSILON))?.sqrt()?)?;
    normalized
        .multiply(&weight.as_type::<f32>()?)?
        .add(&bias.as_type::<f32>()?)?
        .as_type::<f16>()
}

fn alibi(sequence_length: i32) -> MlxResult<Array> {
    let positions = Array::arange::<i32, f32>(None, sequence_length, None)?;
    let distance = positions
        .reshape(&[1, sequence_length])?
        .subtract(&positions.reshape(&[sequence_length, 1])?)?
        .abs()?
        .reshape(&[1, 1, sequence_length, sequence_length])?;
    Array::from_slice(super::weights::alibi_slopes(), &[1, HEAD_COUNT, 1, 1]).multiply(&distance)
}

fn mask_bias(mask: &Array, batch_size: i32, sequence_length: i32) -> MlxResult<Array> {
    Array::from_f32(1.0)
        .subtract(mask)?
        .multiply(Array::from_f32(MASK_BIAS))?
        .reshape(&[batch_size, 1, 1, sequence_length])
}

fn mlx_error(error: Exception) -> ModelError {
    ModelError::internal("MLX forward pass failed").with_cause(error)
}

#[cfg(test)]
mod tests;
