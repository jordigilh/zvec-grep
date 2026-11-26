#![allow(clippy::cast_possible_truncation)]

use std::{env, fs, path::PathBuf};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::MlxEmbeddingModel;
use crate::domain::Content;
use crate::domain::model::{EmbeddingResult, ModelConfig};
use crate::models::catalog::{EmbeddingCatalogEntry, get_embedding_model_catalog_entry};
use crate::models::runtime::ModelComputeRuntime;
use crate::models::spi::{EmbeddingModel, EmbeddingPrepareOptions};

fn entry() -> crate::models::catalog::TransformersConfig {
    let entry = get_embedding_model_catalog_entry("local/jina-embeddings-v2-base-code-mlx")
        .expect("MLX catalog entry is compiled in with --features mlx");
    let EmbeddingCatalogEntry::Mlx(config) = entry else {
        panic!("MLX reference must resolve to the MLX entry");
    };
    config
}

#[test]
fn catalog_entry_reports_its_own_backend_identity() {
    let config = entry();
    let model = MlxEmbeddingModel::new(
        config,
        ModelConfig::default(),
        ModelComputeRuntime::shared(),
    )
    .expect("model");
    let info = model.info();
    info.validate().expect("valid model info");
    let expected = match mlx_rs::Device::try_default()
        .expect("default device")
        .get_type()
        .expect("device type")
    {
        mlx_rs::DeviceType::Gpu => "mlx-gpu",
        mlx_rs::DeviceType::Cpu => "mlx-cpu",
    };
    assert_eq!(info.backend.as_deref(), Some(expected));
    assert_eq!(info.dtype.as_deref(), Some("fp16"));
    assert_eq!(info.dimension, 768);
    assert_eq!(info.max_batch_size, 8);
    assert_eq!(
        info.model.reference(),
        "local/jina-embeddings-v2-base-code-mlx"
    );
}

#[test]
fn the_requested_device_selects_the_recorded_backend() {
    let build = |device: crate::domain::model::Device| {
        MlxEmbeddingModel::new(
            entry(),
            ModelConfig {
                device: Some(device),
                ..ModelConfig::default()
            },
            ModelComputeRuntime::shared(),
        )
    };

    let cpu = build(crate::domain::model::Device::Cpu).expect("cpu model");
    assert_eq!(cpu.info().backend.as_deref(), Some("mlx-cpu"));

    let gpu = build(crate::domain::model::Device::Metal).expect("gpu model");
    assert_eq!(gpu.info().backend.as_deref(), Some("mlx-gpu"));

    for unsupported in [
        crate::domain::model::Device::Cuda,
        crate::domain::model::Device::Vulkan,
    ] {
        let model = build(unsupported).expect("unsupported devices fall back to CPU");
        assert_eq!(model.info().backend.as_deref(), Some("mlx-cpu"));
    }

    let auto = build(crate::domain::model::Device::Auto).expect("auto resolves");
    assert!(matches!(
        auto.info().backend.as_deref(),
        Some("mlx-cpu" | "mlx-gpu")
    ));
}

#[tokio::test]
async fn cancelled_prepare_preserves_the_cancellation_code() {
    let cache = tempfile::tempdir().expect("model cache");
    let model = MlxEmbeddingModel::new(
        entry(),
        ModelConfig {
            cache_dir: Some(cache.path().to_owned()),
            ..ModelConfig::default()
        },
        ModelComputeRuntime::shared(),
    )
    .expect("model");
    let signal = CancellationToken::new();
    signal.cancel();
    let error = model
        .prepare(EmbeddingPrepareOptions {
            signal: Some(signal),
            ..EmbeddingPrepareOptions::default()
        })
        .await
        .expect_err("cancelled prepare");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
}

#[tokio::test]
async fn empty_embed_is_rejected_before_loading() {
    let model = MlxEmbeddingModel::new(
        entry(),
        ModelConfig::default(),
        ModelComputeRuntime::shared(),
    )
    .expect("model");
    let error = model
        .embed(&[], crate::models::spi::EmbeddingOptions::default())
        .await
        .expect_err("empty batch");
    assert_eq!(error.code(), crate::EngineError::INVALID_ARGUMENT);
}

#[tokio::test]
async fn non_text_content_is_rejected_before_loading() {
    let model = MlxEmbeddingModel::new(
        entry(),
        ModelConfig::default(),
        ModelComputeRuntime::shared(),
    )
    .expect("model");
    let image = crate::domain::ImageContent::new(vec![1, 2, 3], crate::domain::FileFormat::Png)
        .expect("image");
    let error = model
        .embed(
            &[vec![Content::Image(image)]],
            crate::models::spi::EmbeddingOptions::default(),
        )
        .await
        .expect_err("image content");
    assert_eq!(error.code(), crate::EngineError::UNSUPPORTED);
}

/// End-to-end numerics gate, mirroring `cached_minilm`.
///
/// Prerequisites (all local, never downloaded by CI):
/// 1. `ZVEC_GREP_TEST_MODEL_CACHE` points at a model cache containing the
///    pinned `jinaai/jina-embeddings-v2-base-code` artifacts at revision
///    `516f4baf13dec4ddddda8631e019b5737c8bc250` (`model.safetensors`,
///    `tokenizer.json`, `config.json`, `tokenizer_config.json`).
/// 2. `ZVEC_GREP_MLX_FIXTURES` points at `jina_mlx_numerics.json`, written by
///    `benchmarks/mlx-gpu/jina_mlx_spike.py --dump-fixtures DIR`.
/// 3. A working MLX Metal device (Apple Silicon, Xcode + Metal Toolchain).
#[tokio::test]
#[ignore = "requires ZVEC_GREP_TEST_MODEL_CACHE with the pinned jina MLX artifacts, \
            ZVEC_GREP_MLX_FIXTURES from jina_mlx_spike.py --dump-fixtures, and a Metal GPU"]
async fn cached_jina_mlx() {
    let cache = PathBuf::from(
        env::var_os("ZVEC_GREP_TEST_MODEL_CACHE")
            .expect("ZVEC_GREP_TEST_MODEL_CACHE must point at the model cache"),
    );
    let fixtures_path = PathBuf::from(
        env::var_os("ZVEC_GREP_MLX_FIXTURES")
            .expect("ZVEC_GREP_MLX_FIXTURES must point at jina_mlx_numerics.json"),
    );
    let model = MlxEmbeddingModel::new(
        entry(),
        ModelConfig {
            cache_dir: Some(cache),
            ..ModelConfig::default()
        },
        ModelComputeRuntime::shared(),
    )
    .expect("model");
    model
        .prepare(EmbeddingPrepareOptions::default())
        .await
        .expect("prepare real MLX model");
    assert!(
        matches!(
            model.device_type().await.expect("worker device"),
            mlx_rs::DeviceType::Gpu
        ),
        "the numerics gate must run on Metal, not a CPU fallback"
    );

    let fixtures = read_fixtures(&fixtures_path);
    let sequences = fixtures
        .ids
        .iter()
        .zip(&fixtures.attention_mask)
        .map(|(ids, mask)| {
            let length = mask.iter().filter(|&&value| value == 1).count();
            ids.iter()
                .take(length)
                .map(|&id| u32::try_from(id).expect("token id fits u32"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let vectors = model
        .embed_token_ids(sequences)
        .await
        .expect("real MLX inference");
    assert_eq!(vectors.len(), fixtures.reference.len());

    let result = EmbeddingResult {
        vectors: vectors.clone(),
        truncated: Vec::new(),
    };
    crate::models::spi::validate_result(model.info(), vectors.len(), &result)
        .expect("validated MLX result");

    for (index, (vector, reference)) in vectors.iter().zip(&fixtures.reference).enumerate() {
        let cosine = cosine_similarity(vector, reference);
        println!("input {index}: cosine {cosine:.8}");
        assert!(
            cosine >= 0.999,
            "input {index}: cosine {cosine} below 0.999"
        );
    }
}

struct Fixtures {
    ids: Vec<Vec<i64>>,
    attention_mask: Vec<Vec<i64>>,
    reference: Vec<Vec<f32>>,
}

fn read_fixtures(path: &std::path::Path) -> Fixtures {
    let bytes = fs::read(path).expect("read MLX numerics fixtures");
    let value: Value = serde_json::from_slice(&bytes).expect("parse MLX numerics fixtures");
    let rows_i64 = |key: &str| -> Vec<Vec<i64>> {
        value[key]
            .as_array()
            .unwrap_or_else(|| panic!("fixture field {key} is an array"))
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap_or_else(|| panic!("fixture field {key} rows are arrays"))
                    .iter()
                    .map(|value| value.as_i64().unwrap_or_else(|| panic!("{key} is integer")))
                    .collect()
            })
            .collect()
    };
    let reference = value["reference"]
        .as_array()
        .expect("fixture reference is an array")
        .iter()
        .map(|row| {
            row.as_array()
                .expect("fixture reference rows are arrays")
                .iter()
                .map(|value| {
                    value
                        .as_f64()
                        .map(|value| value as f32)
                        .expect("reference is numeric")
                })
                .collect()
        })
        .collect();
    Fixtures {
        ids: rows_i64("ids"),
        attention_mask: rows_i64("attention_mask"),
        reference,
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    assert_eq!(left.len(), right.len());
    let dot = left
        .iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    dot / (left_norm * right_norm)
}
