//! Opt-in Apple-GPU embedding backend (issue #74).
//!
//! `mlx_rs::Array` is `!Send`, so the loaded weights and the compiled forward
//! live on a dedicated worker thread ([`MlxWorker`]). The public API only moves
//! plain token ids in and vectors out. The runtime still owns admission and the
//! execution budget; this backend adds one execution thread, not a scheduler.

use std::{env, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use tokenizers::{Tokenizer, TruncationParams, utils::truncation::TruncationDirection};
use tokio::{fs, sync::Mutex};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::Content,
    domain::model::{
        Device, EmbeddingModelInfo, EmbeddingResult, ModelConfig, ModelInfo, ModelProgress,
    },
    models::{
        artifacts::{
            ArtifactHttpClient, ArtifactSource, ModelDownloadProgressReporter, ResolveArtifacts,
            resolve_model_artifacts,
        },
        catalog::TransformersConfig,
        runtime::ModelComputeRuntime,
        spi::{
            EmbeddingModel, EmbeddingOptions, EmbeddingPrepareOptions, ModelError, input_text,
            validate_inputs, validate_result,
        },
    },
};

use super::{forward::JinaMlxForward, worker::MlxWorker};

/// Fixed artifact names for the `JinaBERT` MLX entry.
const WEIGHTS_ARTIFACT: &str = "model.safetensors";
const TOKENIZER_ARTIFACT: &str = "tokenizer.json";

pub(crate) struct MlxEmbeddingModel {
    entry: TransformersConfig,
    info: EmbeddingModelInfo,
    model_cache_dir: PathBuf,
    compute_runtime: ModelComputeRuntime,
    client: ArtifactHttpClient,
    state: Mutex<Option<Arc<LoadedMlxModel>>>,
    /// Device resolved from `ModelConfig::device`; also the device the worker
    /// thread is pinned to, so `info.backend` never overstates the execution path.
    device: mlx_rs::DeviceType,
}

struct LoadedMlxModel {
    tokenizer: Arc<Tokenizer>,
    worker: MlxWorker,
}

/// MLX runs on the Apple GPU or the CPU. `None`/`Auto` take whatever MLX
/// resolves as its default device, `Metal` is explicit, and device kinds MLX
/// has no backend for fall back to the CPU (the recorded identity follows the
/// fallback, so an index can never claim a device it did not use).
fn resolve_device(device: Option<Device>) -> Result<mlx_rs::DeviceType, ModelError> {
    match device {
        // Ask MLX itself: with the metal feature this is the GPU, and on a
        // CPU-only build the same call resolves to the CPU, so neither choice
        // is hard-coded from the request.
        None | Some(Device::Auto) => mlx_rs::Device::try_default()
            .and_then(|device| device.get_type())
            .map_err(|error| {
                ModelError::internal("Unable to resolve the MLX default device").with_cause(error)
            }),
        Some(Device::Metal) => Ok(mlx_rs::DeviceType::Gpu),
        // No MLX backend for these; fall back to CPU like the other local
        // backends do rather than failing the operation.
        Some(Device::Cpu | Device::Cuda | Device::Vulkan) => Ok(mlx_rs::DeviceType::Cpu),
    }
}

impl MlxEmbeddingModel {
    pub(crate) fn new(
        entry: TransformersConfig,
        options: ModelConfig,
        compute_runtime: ModelComputeRuntime,
    ) -> Result<Self, ModelError> {
        let device = resolve_device(options.device)?;
        let backend = match device {
            mlx_rs::DeviceType::Gpu => "mlx-gpu",
            mlx_rs::DeviceType::Cpu => "mlx-cpu",
        };
        let model_cache_dir = options
            .cache_dir
            .or_else(|| env::var_os("ZVEC_GREP_MODEL_CACHE").map(PathBuf::from))
            .unwrap_or_else(default_model_cache_dir);
        Ok(Self {
            entry,
            info: EmbeddingModelInfo {
                model: ModelInfo {
                    provider: entry.provider.to_owned(),
                    name: entry.model.to_owned(),
                    endpoint: None,
                },
                dimension: entry.dimension,
                metric: entry.metric,
                max_batch_size: entry.max_batch_size,
                max_input_tokens: Some(entry.max_input_tokens),
                max_image_bytes: None,
                backend: Some(backend.to_owned()),
                dtype: Some(entry.dtype.to_owned()),
            },
            model_cache_dir,
            compute_runtime,
            client: ArtifactHttpClient::new(),
            state: Mutex::new(None),
            device,
        })
    }

    /// Device the worker actually runs on, queried on the owning thread. The
    /// numerics gate uses it to prove the test ran on Metal, not a CPU fallback.
    #[cfg(test)]
    pub(crate) async fn device_type(&self) -> Result<mlx_rs::DeviceType, ModelError> {
        let loaded = self.ensure_loaded(None, None, 0).await?;
        loaded.worker.device_type().await
    }

    async fn ensure_loaded(
        &self,
        on_progress: Option<Arc<dyn Fn(ModelProgress) + Send + Sync>>,
        signal: Option<&CancellationToken>,
        _execution_concurrency: usize,
    ) -> Result<Arc<LoadedMlxModel>, ModelError> {
        let mut state = self.state.lock().await;
        if let Some(loaded) = &*state {
            return Ok(Arc::clone(loaded));
        }
        let loaded = Arc::new(self.load(on_progress, signal).await?);
        *state = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    async fn load(
        &self,
        on_progress: Option<Arc<dyn Fn(ModelProgress) + Send + Sync>>,
        signal: Option<&CancellationToken>,
    ) -> Result<LoadedMlxModel, ModelError> {
        let reporter = ModelDownloadProgressReporter::new(
            self.entry.reference,
            on_progress,
            self.entry
                .download
                .artifacts
                .iter()
                .map(|artifact| artifact.path.to_owned()),
        );
        reporter.start();
        let hugging_face = self
            .model_cache_dir
            .join(self.entry.download.hugging_face.repo)
            .join(self.entry.download.hugging_face.revision);
        let sources = std::iter::once(ArtifactSource::hugging_face(
            self.entry.download.hugging_face,
            hugging_face,
        ))
        .chain(
            self.entry
                .download
                .fallback_source(&self.model_cache_dir, "mlx"),
        )
        .collect();
        let resolved = resolve_model_artifacts(
            &self.client,
            ResolveArtifacts {
                model: self.entry.reference,
                sources,
                artifacts: self.entry.download.artifacts,
                reporter: &reporter,
                signal,
            },
        )
        .await?;
        let weights_path = resolved
            .paths
            .get(WEIGHTS_ARTIFACT)
            .cloned()
            .ok_or_else(|| {
                ModelError::storage_failure("Resolved MLX safetensors artifact is missing")
            })?;
        let tokenizer_path = resolved
            .paths
            .get(TOKENIZER_ARTIFACT)
            .cloned()
            .ok_or_else(|| {
                ModelError::storage_failure("Resolved MLX tokenizer artifact is missing")
            })?;

        let bytes = fs::read(&tokenizer_path).await.map_err(|error| {
            ModelError::storage_failure("Unable to read MLX tokenizer").with_cause(error)
        })?;
        let tokenizer = Arc::new(prepare_tokenizer(&bytes, self.entry)?);

        let worker = MlxWorker::try_spawn(
            move |_signal| JinaMlxForward::load(&weights_path),
            self.entry.max_batch_size,
            self.device,
        )?;
        worker.prepare(signal.cloned()).await?;
        reporter.finish();
        Ok(LoadedMlxModel { tokenizer, worker })
    }

    /// Test-only entry point for the numerics gate: bypasses tokenization so the
    /// spike's fixture token ids can be replayed through the real forward.
    #[cfg(test)]
    pub(super) async fn embed_token_ids(
        &self,
        sequences: Vec<Vec<u32>>,
    ) -> Result<Vec<Vec<f32>>, ModelError> {
        let loaded = {
            let state = self.state.lock().await;
            state
                .as_ref()
                .cloned()
                .ok_or_else(|| ModelError::internal("MLX model is not prepared"))?
        };
        loaded.worker.embed(sequences, None).await
    }
}

#[async_trait]
impl EmbeddingModel for MlxEmbeddingModel {
    fn info(&self) -> &EmbeddingModelInfo {
        &self.info
    }

    async fn prepare(&self, options: EmbeddingPrepareOptions) -> Result<(), ModelError> {
        self.ensure_loaded(
            options.on_progress,
            options.signal.as_ref(),
            options.execution_concurrency,
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            error
                .wrap(
                    "MLX model preparation failed",
                    Some(format!(
                        "model={} repo={}",
                        self.entry.reference, self.entry.repo
                    )),
                )
                .shared()
        })
    }

    async fn embed(
        &self,
        inputs: &[Vec<Content>],
        options: EmbeddingOptions,
    ) -> Result<EmbeddingResult, ModelError> {
        validate_inputs(&self.info, inputs, |content| {
            matches!(content, Content::Text(_))
        })?;
        let execution_concurrency = options.execution_concurrency.max(1);
        let loaded = self
            .ensure_loaded(
                options.on_progress.clone(),
                options.signal.as_ref(),
                execution_concurrency,
            )
            .await
            .map_err(|error| {
                error.wrap(
                    "MLX embedding failed",
                    Some(format!(
                        "model={} repo={}",
                        self.entry.reference, self.entry.repo
                    )),
                )
            })?;
        let prefix = match options.purpose {
            crate::domain::model::EmbeddingPurpose::Document => self.entry.document_prefix,
            crate::domain::model::EmbeddingPurpose::Query => self.entry.query_prefix,
        };
        let texts = inputs
            .iter()
            .map(|input| {
                let text = input_text(input)?;
                Ok(match prefix {
                    Some(prefix) => format!("{prefix}{text}"),
                    None => text.into_owned(),
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;
        let tokenizer = Arc::clone(&loaded.tokenizer);
        let entry = self.entry;
        let (ids, truncated) = self
            .compute_runtime
            .run(move || encode_texts(&tokenizer, &texts, entry))
            .await??;
        let vectors = loaded.worker.embed(ids, options.signal).await?;
        let result = EmbeddingResult { vectors, truncated };
        validate_result(&self.info, inputs.len(), &result)?;
        Ok(result)
    }
}

fn prepare_tokenizer(bytes: &[u8], entry: TransformersConfig) -> Result<Tokenizer, ModelError> {
    let mut tokenizer =
        Tokenizer::from_bytes(bytes).map_err(|error| tokenization_error(entry, error))?;
    tokenizer.with_padding(None);
    // One token past the cap lets `encode_texts` both detect truncation and
    // keep exactly the capped prefix, matching the transformers backend.
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: entry.max_input_tokens.saturating_add(1),
            direction: TruncationDirection::Right,
            ..TruncationParams::default()
        }))
        .map_err(|error| tokenization_error(entry, error))?;
    Ok(tokenizer)
}

fn encode_texts(
    tokenizer: &Tokenizer,
    texts: &[String],
    entry: TransformersConfig,
) -> Result<(Vec<Vec<u32>>, Vec<usize>), ModelError> {
    let encodings = tokenizer
        .encode_batch(texts.to_vec(), entry.add_special_tokens)
        .map_err(|error| tokenization_error(entry, error))?;
    let mut ids = Vec::with_capacity(encodings.len());
    let mut truncated = Vec::new();
    for (index, encoding) in encodings.iter().enumerate() {
        let tokens = encoding.get_ids();
        if tokens.len() > entry.max_input_tokens {
            truncated.push(index);
        }
        ids.push(
            tokens
                .iter()
                .take(entry.max_input_tokens)
                .copied()
                .collect(),
        );
    }
    Ok((ids, truncated))
}

fn tokenization_error(entry: TransformersConfig, cause: impl std::fmt::Display) -> ModelError {
    ModelError::new(
        crate::EngineError::INTERNAL,
        "MLX tokenization failed",
        Some(format!("model={} repo={}", entry.reference, entry.repo)),
    )
    .with_cause(cause)
}

fn default_model_cache_dir() -> PathBuf {
    env::var_os("ZVEC_GREP_HOME")
        .map(PathBuf::from)
        .or_else(user_home_dir)
        .unwrap_or_else(|| PathBuf::from(".zvec-grep"))
        .join("models")
}

#[cfg(windows)]
fn user_home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(PathBuf::from)
}

#[cfg(not(windows))]
fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".zvec-grep"))
}
