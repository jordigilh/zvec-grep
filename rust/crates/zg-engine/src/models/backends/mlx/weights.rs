//! `JinaBERT` weight layout and safetensors load for the MLX backend.

use std::{collections::HashMap, path::Path};

use half::f16;
use mlx_rs::{Array, ops::indexing::IndexOp};
use tokio_util::sync::CancellationToken;

use crate::models::spi::ModelError;

pub(super) const HIDDEN_SIZE: i32 = 768;
pub(super) const LAYER_COUNT: usize = 12;
pub(super) const HEAD_COUNT: i32 = 12;
pub(super) const HEAD_SIZE: i32 = HIDDEN_SIZE / HEAD_COUNT;
pub(super) const INTERMEDIATE_SIZE: i32 = 3_072;
/// Embedding tensors the compiled forward takes as inputs.
pub(super) const EMBEDDING_INPUTS: usize = 4;

/// `ALiBi` slopes recovered from the shipped ONNX graph: `-(2^-1..2^-8)` then
/// the geometric half-steps `-(2^-0.5, 2^-1.5, 2^-2.5, 2^-3.5)`.
const ALIBI_SLOPES: [f32; HEAD_COUNT as usize] = [
    -0.5,
    -0.25,
    -0.125,
    -0.0625,
    -0.031_25,
    -0.015_625,
    -0.007_812_5,
    -0.003_906_25,
    -0.707_106_77,
    -0.353_553_38,
    -0.176_776_69,
    -0.088_388_346,
];

pub(super) struct JinaWeights {
    pub(super) word_embeddings: Array,
    pub(super) token_type_embeddings: Array,
    pub(super) embedding_ln_weight: Array,
    pub(super) embedding_ln_bias: Array,
    pub(super) layers: Vec<JinaLayerWeights>,
}

pub(super) struct JinaLayerWeights {
    pub(super) query_weight: Array,
    pub(super) query_bias: Array,
    pub(super) key_weight: Array,
    pub(super) key_bias: Array,
    pub(super) value_weight: Array,
    pub(super) value_bias: Array,
    pub(super) query_ln_weight: Array,
    pub(super) query_ln_bias: Array,
    pub(super) key_ln_weight: Array,
    pub(super) key_ln_bias: Array,
    pub(super) output_weight: Array,
    pub(super) output_bias: Array,
    pub(super) output_ln_weight: Array,
    pub(super) output_ln_bias: Array,
    pub(super) layer_norm_1_weight: Array,
    pub(super) layer_norm_1_bias: Array,
    pub(super) gate_weight: Array,
    pub(super) up_weight: Array,
    pub(super) down_weight: Array,
    pub(super) down_bias: Array,
    pub(super) layer_norm_2_weight: Array,
    pub(super) layer_norm_2_bias: Array,
}

/// `ALiBi` slopes laid out for broadcast over `[1, heads, 1, 1]`.
pub(super) fn alibi_slopes() -> &'static [f32] {
    &ALIBI_SLOPES
}

impl JinaWeights {
    /// Flattens every weight into the exact order the compiled forward expects
    /// as graph inputs: embedding tensors first, then each layer in
    /// [`JinaLayerWeights::flatten`] order.
    ///
    /// The first [`EMBEDDING_INPUTS`] arrays are used *outside* the compiled
    /// graph (`run` does the embedding lookup and embedding `LayerNorm` before
    /// compiling), so the graph receives them but only reads from
    /// [`FIRST_LAYER_INPUT`] onwards. They stay in the flat layout so the
    /// ordering has a single definition shared by loader, forward and tests.
    pub(super) fn flatten(&self) -> Vec<Array> {
        let mut inputs =
            Vec::with_capacity(EMBEDDING_INPUTS + LAYER_COUNT * JinaLayerWeights::INPUTS);
        inputs.push(self.word_embeddings.clone());
        inputs.push(self.token_type_embeddings.clone());
        inputs.push(self.embedding_ln_weight.clone());
        inputs.push(self.embedding_ln_bias.clone());
        for layer in &self.layers {
            inputs.extend(layer.flatten());
        }
        inputs
    }

    /// Loads and validates the fp16 `JinaBERT` tensors from a safetensors file.
    ///
    /// The file read cannot be interrupted, so cancellation is checked before
    /// it and again before any conversion work starts.
    pub(super) fn load(path: &Path, signal: &CancellationToken) -> Result<Self, ModelError> {
        if signal.is_cancelled() {
            return Err(ModelError::cancelled("MLX weight load was cancelled"));
        }
        let tensors = Array::load_safetensors(path).map_err(|error| {
            ModelError::storage_failure("Unable to load MLX safetensors weights").with_cause(error)
        })?;
        if signal.is_cancelled() {
            return Err(ModelError::cancelled("MLX weight load was cancelled"));
        }
        Self::from_tensors(&tensors, signal)
    }

    fn from_tensors(
        tensors: &HashMap<String, Array>,
        signal: &CancellationToken,
    ) -> Result<Self, ModelError> {
        let mut layers = Vec::with_capacity(LAYER_COUNT);
        for index in 0..LAYER_COUNT {
            if signal.is_cancelled() {
                return Err(ModelError::cancelled("MLX weight load was cancelled"));
            }
            layers.push(JinaLayerWeights::from_tensors(tensors, index)?);
        }
        Ok(Self {
            word_embeddings: tensor(
                tensors,
                "embeddings.word_embeddings.weight",
                &[-1, HIDDEN_SIZE],
            )?,
            token_type_embeddings: tensor(
                tensors,
                "embeddings.token_type_embeddings.weight",
                &[-1, HIDDEN_SIZE],
            )?,
            embedding_ln_weight: tensor(tensors, "embeddings.LayerNorm.weight", &[HIDDEN_SIZE])?,
            embedding_ln_bias: tensor(tensors, "embeddings.LayerNorm.bias", &[HIDDEN_SIZE])?,
            layers,
        })
    }
}

impl JinaLayerWeights {
    /// Number of tensors per layer, in `flatten` order.
    pub(super) const INPUTS: usize = 22;

    /// Flattens this layer in the exact order the compiled forward indexes.
    fn flatten(&self) -> [Array; Self::INPUTS] {
        [
            self.query_weight.clone(),
            self.query_bias.clone(),
            self.key_weight.clone(),
            self.key_bias.clone(),
            self.value_weight.clone(),
            self.value_bias.clone(),
            self.query_ln_weight.clone(),
            self.query_ln_bias.clone(),
            self.key_ln_weight.clone(),
            self.key_ln_bias.clone(),
            self.output_weight.clone(),
            self.output_bias.clone(),
            self.output_ln_weight.clone(),
            self.output_ln_bias.clone(),
            self.layer_norm_1_weight.clone(),
            self.layer_norm_1_bias.clone(),
            self.gate_weight.clone(),
            self.up_weight.clone(),
            self.down_weight.clone(),
            self.down_bias.clone(),
            self.layer_norm_2_weight.clone(),
            self.layer_norm_2_bias.clone(),
        ]
    }

    fn from_tensors(tensors: &HashMap<String, Array>, index: usize) -> Result<Self, ModelError> {
        let prefix = format!("encoder.layer.{index}");
        let attention = format!("{prefix}.attention.self");
        let output = format!("{prefix}.attention.output");
        let mlp = format!("{prefix}.mlp");
        let fused_name = format!("{mlp}.up_gated_layer.weight");
        Ok(Self {
            query_weight: tensor(
                tensors,
                &format!("{attention}.query.weight"),
                &[HIDDEN_SIZE, HIDDEN_SIZE],
            )?,
            query_bias: tensor(tensors, &format!("{attention}.query.bias"), &[HIDDEN_SIZE])?,
            key_weight: tensor(
                tensors,
                &format!("{attention}.key.weight"),
                &[HIDDEN_SIZE, HIDDEN_SIZE],
            )?,
            key_bias: tensor(tensors, &format!("{attention}.key.bias"), &[HIDDEN_SIZE])?,
            value_weight: tensor(
                tensors,
                &format!("{attention}.value.weight"),
                &[HIDDEN_SIZE, HIDDEN_SIZE],
            )?,
            value_bias: tensor(tensors, &format!("{attention}.value.bias"), &[HIDDEN_SIZE])?,
            query_ln_weight: tensor(
                tensors,
                &format!("{attention}.layer_norm_q.weight"),
                &[HIDDEN_SIZE],
            )?,
            query_ln_bias: tensor(
                tensors,
                &format!("{attention}.layer_norm_q.bias"),
                &[HIDDEN_SIZE],
            )?,
            key_ln_weight: tensor(
                tensors,
                &format!("{attention}.layer_norm_k.weight"),
                &[HIDDEN_SIZE],
            )?,
            key_ln_bias: tensor(
                tensors,
                &format!("{attention}.layer_norm_k.bias"),
                &[HIDDEN_SIZE],
            )?,
            output_weight: tensor(
                tensors,
                &format!("{output}.dense.weight"),
                &[HIDDEN_SIZE, HIDDEN_SIZE],
            )?,
            output_bias: tensor(tensors, &format!("{output}.dense.bias"), &[HIDDEN_SIZE])?,
            output_ln_weight: tensor(
                tensors,
                &format!("{output}.LayerNorm.weight"),
                &[HIDDEN_SIZE],
            )?,
            output_ln_bias: tensor(tensors, &format!("{output}.LayerNorm.bias"), &[HIDDEN_SIZE])?,
            layer_norm_1_weight: tensor(
                tensors,
                &format!("{prefix}.layer_norm_1.weight"),
                &[HIDDEN_SIZE],
            )?,
            layer_norm_1_bias: tensor(
                tensors,
                &format!("{prefix}.layer_norm_1.bias"),
                &[HIDDEN_SIZE],
            )?,
            gate_weight: gate_weight(tensors, &fused_name)?,
            up_weight: up_weight(tensors, &fused_name)?,
            down_weight: tensor(
                tensors,
                &format!("{mlp}.down_layer.weight"),
                &[HIDDEN_SIZE, INTERMEDIATE_SIZE],
            )?,
            down_bias: tensor(tensors, &format!("{mlp}.down_layer.bias"), &[HIDDEN_SIZE])?,
            layer_norm_2_weight: tensor(
                tensors,
                &format!("{prefix}.layer_norm_2.weight"),
                &[HIDDEN_SIZE],
            )?,
            layer_norm_2_bias: tensor(
                tensors,
                &format!("{prefix}.layer_norm_2.bias"),
                &[HIDDEN_SIZE],
            )?,
        })
    }
}

/// The fused GLU projection's gate half (`rows[:3072]`), pre-split at load so the
/// shapeless compiled graph never needs a shape-inferring slice.
fn gate_weight(tensors: &HashMap<String, Array>, name: &str) -> Result<Array, ModelError> {
    let fused = tensor(tensors, name, &[2 * INTERMEDIATE_SIZE, HIDDEN_SIZE])?;
    Ok(fused.index((..INTERMEDIATE_SIZE, ..)))
}

/// The fused GLU projection's value half (`rows[3072:]`).
fn up_weight(tensors: &HashMap<String, Array>, name: &str) -> Result<Array, ModelError> {
    let fused = tensor(tensors, name, &[2 * INTERMEDIATE_SIZE, HIDDEN_SIZE])?;
    Ok(fused.index((INTERMEDIATE_SIZE.., ..)))
}

/// Fetches one tensor, verifies its rank and fixed dimensions, and enforces the
/// fp16 activation policy. `-1` in `expected` marks a free dimension (the
/// embedding vocabulary size is not pinned here).
fn tensor(
    tensors: &HashMap<String, Array>,
    name: &str,
    expected: &[i32],
) -> Result<Array, ModelError> {
    let array = tensors.get(name).ok_or_else(|| {
        ModelError::storage_failure("MLX weights are missing a required tensor")
            .with_cause(format!("tensor={name}"))
    })?;
    let shape = array.shape();
    let matches = shape.len() == expected.len()
        && shape
            .iter()
            .zip(expected)
            .all(|(actual, expected)| *expected < 0 || actual == expected);
    if !matches {
        return Err(
            ModelError::storage_failure("MLX weights have an unexpected tensor shape").with_cause(
                format!("tensor={name} shape={shape:?} expected={expected:?}"),
            ),
        );
    }
    array.as_type::<f16>().map_err(|error| {
        ModelError::storage_failure("Unable to convert MLX weights to fp16").with_cause(error)
    })
}

#[cfg(test)]
mod tests;
