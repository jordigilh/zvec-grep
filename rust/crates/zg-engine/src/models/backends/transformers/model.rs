use crate::domain::Content;
use std::{
    collections::{HashMap, VecDeque},
    env,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex as StdMutex, MutexGuard as StdMutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_trait::async_trait;
use ort::{
    session::{
        Session,
        builder::{GraphOptimizationLevel, PrepackedWeights},
    },
    value::{DynTensor, Tensor},
};
use tokenizers::{
    PaddingParams, PaddingStrategy, Tokenizer, TruncationParams,
    utils::{padding::PaddingDirection, truncation::TruncationDirection},
};
use tokio::{fs, sync::Mutex};
use tokio_util::sync::CancellationToken;

use crate::domain::model::{
    Device, EmbeddingModelInfo, EmbeddingPurpose, EmbeddingResult, ModelConfig, ModelInfo,
    ModelProgress,
};
use crate::models::{
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
};

pub(crate) struct TransformersEmbeddingModel {
    entry: TransformersConfig,
    info: EmbeddingModelInfo,
    model_cache_dir: PathBuf,
    device: Option<Device>,
    compute_runtime: ModelComputeRuntime,
    client: ArtifactHttpClient,
    state: Mutex<Option<Arc<LoadedTransformersModel>>>,
}

struct LoadedTransformersModel {
    tokenizers: PreparedTokenizers,
    sessions: SessionPool,
}

struct PreparedTokenizers {
    probe: Tokenizer,
    inputs: Tokenizer,
}

struct SessionPool {
    model_path: PathBuf,
    prepacked_weights: PrepackedWeights,
    /// Intra-op thread budget for CPU sessions created outside
    /// [`SessionPool::run`], where the runtime execution budget is not known.
    intra_threads: usize,
    state: StdMutex<SessionPoolState>,
    changed: Condvar,
    fallback: StdMutex<()>,
    coreml_batcher: CoreMlBatcher,
}

struct SessionPoolState {
    provider: TransformersExecutionProvider,
    generation: u64,
    creating: usize,
    sessions: Vec<Arc<SessionSlot>>,
}

struct SessionSlot {
    busy: AtomicBool,
    session: StdMutex<Session>,
    /// Intra-op threads this CPU session was created with (`None` for
    /// non-CPU sessions). A session may only serve callers whose execution
    /// limit implies at least this budget.
    intra_threads: Option<usize>,
}

#[derive(Default)]
struct CoreMlBatcher {
    state: StdMutex<CoreMlBatcherState>,
}

#[derive(Default)]
struct CoreMlBatcherState {
    running: bool,
    pending: VecDeque<Arc<CoreMlBatchRequest>>,
}

struct CoreMlBatchRequest {
    prepared: PreparedBatch,
    signal: Option<CancellationToken>,
    result: StdMutex<Option<Result<EmbeddingResult, String>>>,
    changed: Condvar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransformersExecutionProvider {
    Cpu,
    CoreMl,
    WebGpu,
    Cuda,
    DirectMl,
}

#[derive(Clone)]
struct PreparedBatch {
    input_ids: Vec<i64>,
    attention_mask: Vec<i64>,
    token_type_ids: Vec<i64>,
    position_ids: Vec<i64>,
    padding_input_id: i64,
    batch_size: usize,
    sequence_length: usize,
    truncated: Vec<usize>,
}

impl TransformersEmbeddingModel {
    pub(crate) fn new(
        entry: TransformersConfig,
        options: ModelConfig,
        compute_runtime: ModelComputeRuntime,
    ) -> Self {
        let model_cache_dir = options
            .cache_dir
            .or_else(|| env::var_os("ZVEC_GREP_MODEL_CACHE").map(PathBuf::from))
            .unwrap_or_else(default_model_cache_dir);
        Self {
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
            },
            model_cache_dir,
            device: options.device,
            compute_runtime,
            client: ArtifactHttpClient::new(),
            state: Mutex::new(None),
        }
    }

    async fn ensure_loaded(
        &self,
        on_progress: Option<Arc<dyn Fn(ModelProgress) + Send + Sync>>,
        signal: Option<&CancellationToken>,
        execution_concurrency: usize,
    ) -> Result<Arc<LoadedTransformersModel>, ModelError> {
        let mut state = self.state.lock().await;
        if let Some(loaded) = &*state {
            return Ok(Arc::clone(loaded));
        }
        let loaded = Arc::new(
            self.load(on_progress, signal, execution_concurrency)
                .await?,
        );
        *state = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    async fn load(
        &self,
        on_progress: Option<Arc<dyn Fn(ModelProgress) + Send + Sync>>,
        signal: Option<&CancellationToken>,
        execution_concurrency: usize,
    ) -> Result<LoadedTransformersModel, ModelError> {
        let model_artifact = onnx_artifact(self.entry)?;
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
                .fallback_source(&self.model_cache_dir, "transformers-js"),
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
        let model_path = resolved.paths.get(model_artifact).cloned().ok_or_else(|| {
            ModelError::storage_failure("Resolved Transformers model artifact is missing")
        })?;
        let tokenizer_path = resolved
            .paths
            .get("tokenizer.json")
            .cloned()
            .ok_or_else(|| {
                ModelError::storage_failure("Resolved Transformers tokenizer artifact is missing")
            })?;

        let tokenizer = fs::read(&tokenizer_path).await.map_err(|error| {
            ModelError::storage_failure("Unable to read Transformers tokenizer").with_cause(error)
        })?;
        let tokenizer = Tokenizer::from_bytes(&tokenizer).map_err(|error| {
            ModelError::new(
                crate::EngineError::STORAGE_FAILURE,
                "Transformers tokenization failed",
                Some(format!(
                    "model={} repo={}",
                    self.entry.reference, self.entry.repo
                )),
            )
            .with_cause(error)
        })?;
        let tokenizers = prepare_tokenizers(tokenizer, self.entry)?;
        let device = self.device;
        let reporter_for_session = reporter.clone();
        let sessions = self
            .compute_runtime
            .run(move || {
                SessionPool::load(
                    model_path,
                    device,
                    execution_concurrency,
                    &reporter_for_session,
                )
            })
            .await??;
        reporter.finish();
        Ok(LoadedTransformersModel {
            tokenizers,
            sessions,
        })
    }
}

#[async_trait]
impl EmbeddingModel for TransformersEmbeddingModel {
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
                    "Transformers model preparation failed",
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
                    "Transformers embedding failed",
                    Some(format!(
                        "model={} repo={}",
                        self.entry.reference, self.entry.repo
                    )),
                )
            })?;
        let purpose = options.purpose;
        let prefix = match purpose {
            EmbeddingPurpose::Document => self.entry.document_prefix,
            EmbeddingPurpose::Query => self.entry.query_prefix,
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
        let entry = self.entry;
        let signal = options.signal;
        let on_progress = options.on_progress;
        let result = self
            .compute_runtime
            .run(move || {
                embed_batch(
                    &loaded,
                    &texts,
                    entry,
                    execution_concurrency,
                    signal.as_ref(),
                    on_progress.as_ref(),
                )
            })
            .await??;
        validate_result(&self.info, inputs.len(), &result)?;
        Ok(result)
    }
}

impl SessionPool {
    fn load(
        model_path: PathBuf,
        device: Option<Device>,
        execution_concurrency: usize,
        reporter: &ModelDownloadProgressReporter,
    ) -> Result<Self, ModelError> {
        let prepacked_weights = PrepackedWeights::new();
        let intra_threads = cpu_intra_threads(execution_concurrency);
        let requested = resolve_execution_provider(device);
        if cfg!(target_os = "macos")
            && matches!(device, Some(Device::Metal))
            && requested == TransformersExecutionProvider::Cpu
        {
            let warning = "Metal was requested for the Transformers ONNX model, but the available CoreML path regresses throughput and memory; using ORT CPU instead.";
            if !reporter.warning(warning) {
                tracing::warn!("{warning}");
            }
        }
        let (session, provider) = if requested == TransformersExecutionProvider::Cpu {
            (
                load_session(
                    &model_path,
                    requested,
                    &prepacked_weights,
                    Some(intra_threads),
                )?,
                TransformersExecutionProvider::Cpu,
            )
        } else {
            match load_session(&model_path, requested, &prepacked_weights, None) {
                Ok(session) => (session, requested),
                Err(error) => {
                    let warning = format!(
                        "Transformers {} embedding initialization failed ({}), falling back to CPU.",
                        requested.name(),
                        error
                    );
                    if !reporter.warning(warning.clone()) {
                        tracing::warn!("{warning}");
                    }
                    (
                        load_session(
                            &model_path,
                            TransformersExecutionProvider::Cpu,
                            &prepacked_weights,
                            Some(intra_threads),
                        )?,
                        TransformersExecutionProvider::Cpu,
                    )
                }
            }
        };
        Ok(Self {
            model_path,
            prepacked_weights,
            intra_threads,
            state: StdMutex::new(SessionPoolState {
                provider,
                generation: 0,
                creating: 0,
                sessions: vec![Arc::new(SessionSlot {
                    busy: AtomicBool::new(false),
                    session: StdMutex::new(session),
                    intra_threads: if provider == TransformersExecutionProvider::Cpu {
                        Some(intra_threads)
                    } else {
                        None
                    },
                })],
            }),
            changed: Condvar::new(),
            fallback: StdMutex::new(()),
            coreml_batcher: CoreMlBatcher::default(),
        })
    }

    fn run<T>(
        &self,
        max_sessions: usize,
        inference: impl Fn(&mut Session) -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        let max_sessions = max_sessions.max(1);
        loop {
            let mut state = lock_std_mutex(&self.state);
            // One cached runtime serves leases with different execution limits
            // (indexing and queries), so sessions created for an earlier lease
            // can outlive that lease's budget. The desired budget and both the
            // retirement sweep and the claim below run under this lock, but a
            // completing caller clears `busy` without it, so a slot that was
            // busy during the sweep can be idle by the claim. The claim
            // therefore re-checks the budget itself: an oversized session is
            // never handed out, it is retired by a later sweep instead.
            // Reusing an oversized session would oversubscribe cores;
            // undersized sessions stay reusable, which can only underutilize
            // the machine, never exceed the budget.
            let desired_budget = if state.provider == TransformersExecutionProvider::Cpu {
                Some(cpu_intra_threads(max_sessions))
            } else {
                None
            };
            if let Some(desired) = desired_budget {
                let before = state.sessions.len();
                state.sessions.retain(|slot| {
                    slot.busy.load(Ordering::Acquire)
                        || cpu_session_within_budget(slot.intra_threads, desired)
                });
                if state.sessions.len() != before {
                    self.changed.notify_all();
                }
            }
            if let Some(slot) = state.sessions.iter().find_map(|slot| {
                (cpu_session_claimable(slot.intra_threads, desired_budget)
                    && slot
                        .busy
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok())
                .then(|| Arc::clone(slot))
            }) {
                drop(state);
                let mut session = lock_session(&slot.session);
                let result = inference(&mut session);
                drop(session);
                slot.busy.store(false, Ordering::Release);
                self.changed.notify_one();
                return result;
            }

            let max_sessions = physical_session_limit(state.provider, max_sessions);
            if state.sessions.len() + state.creating < max_sessions {
                let provider = state.provider;
                let generation = state.generation;
                let intra_threads = if provider == TransformersExecutionProvider::Cpu {
                    Some(cpu_intra_threads(max_sessions))
                } else {
                    None
                };
                state.creating += 1;
                drop(state);
                let created = load_session(
                    &self.model_path,
                    provider,
                    &self.prepacked_weights,
                    intra_threads,
                );
                let mut state = lock_std_mutex(&self.state);
                state.creating = state.creating.saturating_sub(1);
                match created {
                    Ok(session) if state.generation == generation && state.provider == provider => {
                        state.sessions.push(Arc::new(SessionSlot {
                            busy: AtomicBool::new(false),
                            session: StdMutex::new(session),
                            intra_threads,
                        }));
                        self.changed.notify_all();
                        continue;
                    }
                    Ok(_) => {
                        self.changed.notify_all();
                        continue;
                    }
                    Err(error) => {
                        self.changed.notify_all();
                        return Err(error);
                    }
                }
            }

            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            drop(state);
        }
    }

    fn run_coreml_batched(
        &self,
        prepared: PreparedBatch,
        entry: TransformersConfig,
        execution_concurrency: usize,
        signal: Option<&CancellationToken>,
    ) -> Result<EmbeddingResult, ModelError> {
        check_cancelled(signal)?;
        let request = Arc::new(CoreMlBatchRequest {
            prepared,
            signal: signal.cloned(),
            result: StdMutex::new(None),
            changed: Condvar::new(),
        });
        let is_leader = {
            let mut state = lock_std_mutex(&self.coreml_batcher.state);
            state.pending.push_back(Arc::clone(&request));
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };

        if is_leader {
            // Give callers admitted by the same runtime wave a small window to
            // join one CoreML invocation. This keeps one compiled CoreML graph
            // while still turning user concurrency into useful device work.
            if execution_concurrency > 1 && request.prepared.batch_size < entry.max_batch_size {
                thread::sleep(Duration::from_millis(1));
            }
            self.drain_coreml_batches(entry, entry.max_batch_size);
        }

        let mut result = lock_std_mutex(&request.result);
        while result.is_none() {
            result = request
                .changed
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
        result
            .take()
            .expect("CoreML batch request result was checked")
            .map_err(ModelError::internal)
    }

    fn drain_coreml_batches(&self, entry: TransformersConfig, max_batch_size: usize) {
        loop {
            let requests = {
                let mut state = lock_std_mutex(&self.coreml_batcher.state);
                let mut requests = Vec::new();
                let mut vector_count = 0_usize;
                while let Some(request) = state.pending.front() {
                    let request_size = request.prepared.batch_size;
                    if !requests.is_empty()
                        && vector_count.saturating_add(request_size) > max_batch_size
                    {
                        break;
                    }
                    vector_count = vector_count.saturating_add(request_size);
                    requests.push(
                        state
                            .pending
                            .pop_front()
                            .expect("CoreML pending request disappeared"),
                    );
                }
                if requests.is_empty() {
                    state.running = false;
                    return;
                }
                requests
            };

            let active = requests
                .iter()
                .filter(|request| {
                    !request
                        .signal
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                })
                .cloned()
                .collect::<Vec<_>>();
            if active.is_empty() {
                for request in requests {
                    complete_coreml_request(
                        &request,
                        Err("Transformers embedding was cancelled".to_owned()),
                    );
                }
                continue;
            }

            let merged = merge_prepared_batches(
                &active
                    .iter()
                    .map(|request| &request.prepared)
                    .collect::<Vec<_>>(),
            );
            let result = merged.and_then(|prepared| {
                self.run(1, |session| run_session(session, &prepared, entry, None))
            });
            match result {
                Ok(result) => complete_coreml_batch(&requests, &active, &result),
                Err(error) => {
                    let message = error.to_string();
                    for request in requests {
                        let result = if request
                            .signal
                            .as_ref()
                            .is_some_and(CancellationToken::is_cancelled)
                        {
                            Err("Transformers embedding was cancelled".to_owned())
                        } else {
                            Err(message.clone())
                        };
                        complete_coreml_request(&request, result);
                    }
                }
            }
        }
    }

    fn provider(&self) -> TransformersExecutionProvider {
        lock_std_mutex(&self.state).provider
    }

    fn fallback_to_cpu(&self) -> Result<bool, ModelError> {
        let _fallback = lock_std_mutex(&self.fallback);
        if self.provider() == TransformersExecutionProvider::Cpu {
            return Ok(false);
        }
        let session = load_session(
            &self.model_path,
            TransformersExecutionProvider::Cpu,
            &self.prepacked_weights,
            Some(self.intra_threads),
        )?;
        let mut state = lock_std_mutex(&self.state);
        state.provider = TransformersExecutionProvider::Cpu;
        state.generation = state.generation.wrapping_add(1);
        state.sessions = vec![Arc::new(SessionSlot {
            busy: AtomicBool::new(false),
            session: StdMutex::new(session),
            intra_threads: Some(self.intra_threads),
        })];
        self.changed.notify_all();
        Ok(true)
    }
}

const fn physical_session_limit(
    provider: TransformersExecutionProvider,
    requested: usize,
) -> usize {
    match provider {
        TransformersExecutionProvider::CoreMl => 1,
        TransformersExecutionProvider::Cpu
        | TransformersExecutionProvider::WebGpu
        | TransformersExecutionProvider::Cuda
        | TransformersExecutionProvider::DirectMl => requested,
    }
}

/// Intra-op threads per CPU session so `execution_concurrency` pooled sessions
/// together stay within the machine's core budget. Matches the llama.cpp
/// backend's `threads_per_context` policy.
fn cpu_intra_threads(execution_concurrency: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    (cores / execution_concurrency.max(1)).max(1)
}

/// Whether a pooled CPU session created with `slot_threads` intra-op threads
/// may serve a caller whose execution limit implies `desired` threads.
/// Oversized sessions must be retired rather than reused so a lease with a
/// smaller limit cannot run on threads a previous lease granted; undersized
/// sessions stay reusable (within budget, at most underutilizing the machine).
const fn cpu_session_within_budget(slot_threads: Option<usize>, desired: usize) -> bool {
    match slot_threads {
        Some(threads) => threads <= desired,
        None => true,
    }
}

/// Whether a pooled session may be claimed by a caller whose execution limit
/// implies `desired_budget` intra-op threads (`None` for non-CPU pools, where
/// sessions carry no budget). Unlike the retirement sweep, this runs before
/// the busy claim, so a session that goes idle mid-sweep (its completing
/// caller clears `busy` without the pool lock) still cannot be handed out
/// with an oversized budget.
const fn cpu_session_claimable(slot_threads: Option<usize>, desired_budget: Option<usize>) -> bool {
    match (slot_threads, desired_budget) {
        (Some(threads), Some(desired)) => threads <= desired,
        _ => true,
    }
}

impl TransformersExecutionProvider {
    const fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CoreMl => "coreml",
            Self::WebGpu => "webgpu",
            Self::Cuda => "cuda",
            Self::DirectMl => "dml",
        }
    }
}

fn resolve_execution_provider(device: Option<Device>) -> TransformersExecutionProvider {
    match device {
        None | Some(Device::Cpu) => TransformersExecutionProvider::Cpu,
        Some(Device::Metal | Device::Auto) if cfg!(target_os = "macos") => {
            TransformersExecutionProvider::Cpu
        }
        Some(Device::Metal) => TransformersExecutionProvider::CoreMl,
        Some(Device::Cuda) => TransformersExecutionProvider::Cuda,
        // ORT's CPU/MLAS path is both faster and substantially smaller than
        // CoreML for the catalog's quantized ONNX model on Apple Silicon.
        Some(Device::Auto) if cfg!(target_os = "windows") => {
            TransformersExecutionProvider::DirectMl
        }
        Some(Device::Auto) if cfg!(all(target_os = "linux", target_arch = "x86_64")) => {
            TransformersExecutionProvider::Cuda
        }
        Some(Device::Vulkan | Device::Auto) => TransformersExecutionProvider::WebGpu,
    }
}

fn load_session(
    path: &Path,
    provider: TransformersExecutionProvider,
    prepacked_weights: &PrepackedWeights,
    intra_threads: Option<usize>,
) -> Result<Session, ModelError> {
    let builder = Session::builder()
        .map_err(|error| {
            ModelError::internal("Unable to configure ONNX embedding model").with_cause(error)
        })?
        .with_prepacked_weights(prepacked_weights)
        .map_err(|error| {
            ModelError::internal("Unable to share ONNX prepacked weights").with_cause(error)
        })?;
    let builder = if provider == TransformersExecutionProvider::Cpu {
        // Without an explicit intra-op budget every pooled session defaults to
        // one thread per core, so `execution_concurrency` concurrent sessions
        // oversubscribe the machine. Mirror the llama.cpp backend and divide
        // the available cores across the requested session count.
        let threads = intra_threads.unwrap_or_else(|| cpu_intra_threads(1));
        builder.with_intra_threads(threads).map_err(|error| {
            ModelError::internal("Unable to configure ONNX embedding model").with_cause(error)
        })?
    } else {
        builder
    };
    let builder = configure_execution_provider(builder, provider)?;
    let mut builder = builder
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|error| {
            ModelError::internal("Unable to configure ONNX embedding model").with_cause(error)
        })?;
    builder.commit_from_file(path).map_err(|error| {
        ModelError::storage_failure(format!(
            "Unable to load {} ONNX embedding model",
            provider.name()
        ))
        .with_cause(error)
    })
}

fn configure_execution_provider(
    builder: ort::session::builder::SessionBuilder,
    provider: TransformersExecutionProvider,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    match provider {
        TransformersExecutionProvider::Cpu => Ok(builder),
        TransformersExecutionProvider::CoreMl => configure_coreml(builder),
        TransformersExecutionProvider::WebGpu => configure_webgpu(builder),
        TransformersExecutionProvider::Cuda => configure_cuda(builder),
        TransformersExecutionProvider::DirectMl => configure_directml(builder),
    }
}

#[cfg(target_os = "macos")]
fn configure_coreml(
    builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    builder
        .with_execution_providers([ort::ep::CoreML::default().build().error_on_failure()])
        .map_err(|error| {
            ModelError::internal("Unable to configure CoreML ONNX embedding model")
                .with_cause(error)
        })
}

#[cfg(not(target_os = "macos"))]
fn configure_coreml(
    _builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    Err(ModelError::unsupported(
        "CoreML ONNX accelerator is unavailable in this build",
    ))
}

#[cfg(feature = "vulkan")]
fn configure_webgpu(
    builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    builder
        .with_execution_providers([ort::ep::WebGPU::default().build().error_on_failure()])
        .map_err(|error| {
            ModelError::internal("Unable to configure WebGPU ONNX embedding model")
                .with_cause(error)
        })
}

#[cfg(not(feature = "vulkan"))]
fn configure_webgpu(
    _builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    Err(ModelError::unsupported(
        "WebGPU ONNX accelerator is unavailable in this build",
    ))
}

#[cfg(feature = "cuda")]
fn configure_cuda(
    builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    builder
        .with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()])
        .map_err(|error| {
            ModelError::internal("Unable to configure CUDA ONNX embedding model").with_cause(error)
        })
}

#[cfg(not(feature = "cuda"))]
fn configure_cuda(
    _builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    Err(ModelError::unsupported(
        "CUDA ONNX accelerator is unavailable in this build",
    ))
}

#[cfg(target_os = "windows")]
fn configure_directml(
    builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    builder
        .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
        .map_err(|error| {
            ModelError::internal("Unable to configure DirectML ONNX embedding model")
                .with_cause(error)
        })
}

#[cfg(not(target_os = "windows"))]
fn configure_directml(
    _builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder, ModelError> {
    Err(ModelError::unsupported(
        "DirectML ONNX accelerator is unavailable in this build",
    ))
}

/// Upper bound on padded tokens (`batch_size x longest sequence`) per CPU
/// inference call. At 8192-token contexts an unbounded batch needs
/// `N x 3.2 GB` of attention scores; this keeps per-call memory flat while
/// still amortizing inference over many short fragments. A single sequence
/// longer than the budget still runs alone.
const MAX_BATCH_TOKENS: usize = 4096;

fn embed_batch(
    loaded: &LoadedTransformersModel,
    texts: &[String],
    entry: TransformersConfig,
    execution_concurrency: usize,
    signal: Option<&CancellationToken>,
    on_progress: Option<&Arc<dyn Fn(ModelProgress) + Send + Sync>>,
) -> Result<EmbeddingResult, ModelError> {
    check_cancelled(signal)?;
    let provider = loaded.sessions.provider();
    if provider == TransformersExecutionProvider::Cpu {
        return run_cpu_batched(loaded, texts, entry, execution_concurrency, signal);
    }
    let prepared = prepare_batch(&loaded.tokenizers, texts, entry)?;
    check_cancelled(signal)?;
    let first = if provider == TransformersExecutionProvider::CoreMl {
        loaded
            .sessions
            .run_coreml_batched(prepared.clone(), entry, execution_concurrency, signal)
    } else {
        loaded.sessions.run(execution_concurrency, |session| {
            run_session(session, &prepared, entry, signal)
        })
    };
    match first {
        Ok(result) => Ok(result),
        Err(error) => {
            let warning = format!(
                "Transformers {} embedding inference failed ({}), falling back to CPU.",
                provider.name(),
                error
            );
            if let Some(on_progress) = on_progress {
                on_progress(ModelProgress::Warning {
                    model: entry.reference.to_owned(),
                    message: warning,
                });
            } else {
                tracing::warn!("{warning}");
            }
            loaded.sessions.fallback_to_cpu()?;
            run_cpu_batched(loaded, texts, entry, execution_concurrency, signal)
        }
    }
}

/// CPU inference split into token-budgeted sub-batches so one long fragment
/// cannot stretch (or OOM) the whole batch. Vectors are reassembled in the
/// original input order. The probe encodings are computed once for the whole
/// request and reused by every sub-batch, so each text is probe-tokenized
/// exactly once per embed call.
fn run_cpu_batched(
    loaded: &LoadedTransformersModel,
    texts: &[String],
    entry: TransformersConfig,
    execution_concurrency: usize,
    signal: Option<&CancellationToken>,
) -> Result<EmbeddingResult, ModelError> {
    let probe = probe_encodings(&loaded.tokenizers, texts, entry)?;
    let token_lengths = probe.iter().map(probe_token_count).collect::<Vec<_>>();
    embed_sub_batches(
        texts,
        &token_lengths,
        entry,
        signal,
        |indexes, sub_texts| {
            let sub_probe = indexes
                .iter()
                .map(|&index| probe[index].clone())
                .collect::<Vec<_>>();
            let prepared =
                prepare_batch_from_probe(&loaded.tokenizers, sub_texts, entry, &sub_probe)?;
            loaded.sessions.run(execution_concurrency, |session| {
                run_session(session, &prepared, entry, signal)
            })
        },
    )
}

/// Encodings from the no-padding probe tokenizer (truncating one token past
/// the model limit), used to pack sub-batches and to detect truncation.
fn probe_encodings(
    tokenizers: &PreparedTokenizers,
    texts: &[String],
    entry: TransformersConfig,
) -> Result<Vec<tokenizers::Encoding>, ModelError> {
    tokenizers
        .probe
        .encode_batch(texts.to_vec(), entry.add_special_tokens)
        .map_err(|error| tokenization_error(entry, error))
}

fn probe_token_count(encoding: &tokenizers::Encoding) -> usize {
    encoding
        .get_attention_mask()
        .iter()
        .filter(|&&value| value != 0)
        .count()
}

/// Greedily pack input indexes into sub-batches constrained by both
/// `max_batch_size` and `MAX_BATCH_TOKENS` (`batch_size x longest sequence`,
/// padding included). Indexes are sorted by descending token length so short
/// fragments batch together instead of being padded to the longest input.
/// Every index appears in exactly one sub-batch; a single sequence longer
/// than the token budget still forms its own sub-batch.
fn pack_token_budgeted_batches(
    token_lengths: &[usize],
    max_batch_size: usize,
    token_budget: usize,
) -> Vec<Vec<usize>> {
    let max_batch_size = max_batch_size.max(1);
    let mut order = (0..token_lengths.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| token_lengths[b].cmp(&token_lengths[a]).then(a.cmp(&b)));
    let mut batches = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut longest = 0_usize;
    for index in order {
        // An empty input still occupies one padded position in its batch.
        let length = token_lengths[index].max(1);
        if !current.is_empty()
            && (current.len() >= max_batch_size
                || current.len().saturating_add(1).saturating_mul(longest) > token_budget)
        {
            batches.push(std::mem::take(&mut current));
            longest = 0;
        }
        longest = longest.max(length);
        current.push(index);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Runs each packed sub-batch through `embed_sub_batch`, reassembling vectors
/// and truncation indexes in the original input order. Cancellation is
/// re-checked before every sub-batch. The sub-batch callback receives both
/// the original input indexes and their texts so callers can reuse
/// per-request state (such as precomputed probe encodings) for the sub-batch.
fn embed_sub_batches(
    texts: &[String],
    token_lengths: &[usize],
    entry: TransformersConfig,
    signal: Option<&CancellationToken>,
    mut embed_sub_batch: impl FnMut(&[usize], &[String]) -> Result<EmbeddingResult, ModelError>,
) -> Result<EmbeddingResult, ModelError> {
    let sub_batches =
        pack_token_budgeted_batches(token_lengths, entry.max_batch_size, MAX_BATCH_TOKENS);
    let mut vectors = (0..texts.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut truncated = Vec::new();
    for indexes in sub_batches {
        check_cancelled(signal)?;
        let sub_texts = indexes
            .iter()
            .map(|&index| texts[index].clone())
            .collect::<Vec<_>>();
        let result = embed_sub_batch(&indexes, &sub_texts)?;
        if result.vectors.len() != indexes.len() {
            return Err(ModelError::internal(
                "Transformers sub-batch returned an unexpected vector count",
            ));
        }
        for (local_index, &original_index) in indexes.iter().enumerate() {
            vectors[original_index].clone_from(&result.vectors[local_index]);
            if result.truncated.contains(&local_index) {
                truncated.push(original_index);
            }
        }
    }
    Ok(EmbeddingResult { vectors, truncated })
}

fn merge_prepared_batches(batches: &[&PreparedBatch]) -> Result<PreparedBatch, ModelError> {
    let sequence_length = batches
        .iter()
        .map(|batch| batch.sequence_length)
        .max()
        .unwrap_or_default();
    let batch_size = batches.iter().map(|batch| batch.batch_size).sum::<usize>();
    if sequence_length == 0 || batch_size == 0 {
        return Err(ModelError::internal(
            "Unable to merge an empty CoreML embedding batch",
        ));
    }

    let capacity = batch_size.saturating_mul(sequence_length);
    let mut input_ids = Vec::with_capacity(capacity);
    let mut attention_mask = Vec::with_capacity(capacity);
    let mut token_type_ids = Vec::with_capacity(capacity);
    let mut position_ids = Vec::with_capacity(capacity);
    let mut truncated = Vec::new();
    let mut batch_offset = 0_usize;
    for batch in batches {
        for row in 0..batch.batch_size {
            append_padded_row(
                &mut input_ids,
                &batch.input_ids,
                row,
                batch.sequence_length,
                sequence_length,
                batch.padding_input_id,
            );
            append_padded_row(
                &mut attention_mask,
                &batch.attention_mask,
                row,
                batch.sequence_length,
                sequence_length,
                0,
            );
            append_padded_row(
                &mut token_type_ids,
                &batch.token_type_ids,
                row,
                batch.sequence_length,
                sequence_length,
                0,
            );
            append_padded_row(
                &mut position_ids,
                &batch.position_ids,
                row,
                batch.sequence_length,
                sequence_length,
                0,
            );
        }
        truncated.extend(batch.truncated.iter().map(|index| batch_offset + index));
        batch_offset += batch.batch_size;
    }

    Ok(PreparedBatch {
        input_ids,
        attention_mask,
        token_type_ids,
        position_ids,
        padding_input_id: batches[0].padding_input_id,
        batch_size,
        sequence_length,
        truncated,
    })
}

fn append_padded_row(
    destination: &mut Vec<i64>,
    source: &[i64],
    row: usize,
    source_width: usize,
    destination_width: usize,
    padding: i64,
) {
    let start = row.saturating_mul(source_width);
    let end = start.saturating_add(source_width);
    destination.extend_from_slice(&source[start..end]);
    destination.resize(
        destination.len() + destination_width - source_width,
        padding,
    );
}

fn complete_coreml_batch(
    requests: &[Arc<CoreMlBatchRequest>],
    active: &[Arc<CoreMlBatchRequest>],
    result: &EmbeddingResult,
) {
    let expected = active
        .iter()
        .map(|request| request.prepared.batch_size)
        .sum::<usize>();
    if result.vectors.len() != expected {
        let error = format!(
            "CoreML merged batch returned {} vectors for {expected} inputs",
            result.vectors.len()
        );
        for request in requests {
            complete_coreml_request(request, Err(error.clone()));
        }
        return;
    }

    let mut offset = 0_usize;
    for request in requests {
        let was_active = active
            .iter()
            .any(|active_request| Arc::ptr_eq(active_request, request));
        if !was_active {
            complete_coreml_request(
                request,
                Err("Transformers embedding was cancelled".to_owned()),
            );
            continue;
        }
        let end = offset + request.prepared.batch_size;
        let request_result = if request
            .signal
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            Err("Transformers embedding was cancelled".to_owned())
        } else {
            Ok(EmbeddingResult {
                vectors: result.vectors[offset..end].to_vec(),
                truncated: request.prepared.truncated.clone(),
            })
        };
        complete_coreml_request(request, request_result);
        offset = end;
    }
}

fn complete_coreml_request(request: &CoreMlBatchRequest, result: Result<EmbeddingResult, String>) {
    *lock_std_mutex(&request.result) = Some(result);
    request.changed.notify_one();
}

fn run_session(
    session: &mut Session,
    prepared: &PreparedBatch,
    entry: TransformersConfig,
    signal: Option<&CancellationToken>,
) -> Result<EmbeddingResult, ModelError> {
    check_cancelled(signal)?;
    let mut inputs = HashMap::<String, DynTensor>::new();
    for input in session.inputs() {
        let data = match input.name() {
            "input_ids" => prepared.input_ids.clone(),
            "attention_mask" => prepared.attention_mask.clone(),
            "token_type_ids" => prepared.token_type_ids.clone(),
            "position_ids" => prepared.position_ids.clone(),
            name => {
                return Err(ModelError::internal(format!(
                    "Unsupported ONNX embedding input: {name}"
                )));
            }
        };
        let tensor = Tensor::from_array(([prepared.batch_size, prepared.sequence_length], data))
            .map_err(|error| {
                ModelError::internal("Unable to create ONNX input tensor").with_cause(error)
            })?;
        inputs.insert(input.name().to_owned(), tensor.upcast());
    }
    let outputs = session.run(inputs).map_err(|error| {
        ModelError::internal("ONNX embedding inference failed").with_cause(error)
    })?;
    let named_output = outputs
        .get("last_hidden_state")
        .or_else(|| outputs.get("token_embeddings"))
        .or_else(|| outputs.get("sentence_embedding"));
    let output = if let Some(output) = named_output {
        output
    } else if outputs.len() > 0 {
        &outputs[0]
    } else {
        return Err(ModelError::internal(
            "ONNX embedding model returned no tensor",
        ));
    };
    let (shape, data) = output.try_extract_tensor::<f32>().map_err(|error| {
        ModelError::new(
            crate::EngineError::INTERNAL,
            "Transformers returned an unexpected tensor",
            None,
        )
        .with_cause(error)
    })?;
    let shape = shape.iter().copied().collect::<Vec<_>>();
    let data = data.to_vec();
    drop(outputs);
    check_cancelled(signal)?;
    let vectors = pool_output(
        &shape,
        &data,
        &prepared.attention_mask,
        prepared.batch_size,
        prepared.sequence_length,
        entry,
    )?;
    Ok(EmbeddingResult {
        vectors,
        truncated: prepared.truncated.clone(),
    })
}

fn prepare_tokenizers(
    mut tokenizer: Tokenizer,
    entry: TransformersConfig,
) -> Result<PreparedTokenizers, ModelError> {
    // WordPiece cloning copies its vocabulary. Configure both immutable variants
    // once per loaded model so requests can share them without cloning or locks.
    tokenizer.with_padding(None);
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: entry.max_input_tokens + 1,
            direction: TruncationDirection::Right,
            ..TruncationParams::default()
        }))
        .map_err(|error| tokenization_error(entry, error))?;
    let probe = tokenizer.clone();
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: entry.max_input_tokens,
            direction: TruncationDirection::Right,
            ..TruncationParams::default()
        }))
        .map_err(|error| tokenization_error(entry, error))?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        direction: PaddingDirection::Right,
        ..PaddingParams::default()
    }));
    Ok(PreparedTokenizers {
        probe,
        inputs: tokenizer,
    })
}

fn prepare_batch(
    tokenizers: &PreparedTokenizers,
    texts: &[String],
    entry: TransformersConfig,
) -> Result<PreparedBatch, ModelError> {
    let probe = probe_encodings(tokenizers, texts, entry)?;
    prepare_batch_from_probe(tokenizers, texts, entry, &probe)
}

/// Builds the padded inference batch, taking the probe encodings for
/// `texts` (in input order) from the caller so the CPU path can compute them
/// once per request instead of once per sub-batch.
fn prepare_batch_from_probe(
    tokenizers: &PreparedTokenizers,
    texts: &[String],
    entry: TransformersConfig,
    probe: &[tokenizers::Encoding],
) -> Result<PreparedBatch, ModelError> {
    let truncated = probe
        .iter()
        .enumerate()
        .filter_map(|(index, encoding)| {
            (probe_token_count(encoding) > entry.max_input_tokens).then_some(index)
        })
        .collect::<Vec<_>>();

    let padding_input_id = i64::from(PaddingParams::default().pad_id);
    let encodings = tokenizers
        .inputs
        .encode_batch(texts.to_vec(), entry.add_special_tokens)
        .map_err(|error| tokenization_error(entry, error))?;
    let encoded_length = encodings
        .first()
        .map(tokenizers::Encoding::len)
        .unwrap_or_default();
    if encodings.is_empty()
        || encodings
            .iter()
            .any(|encoding| encoding.len() != encoded_length)
    {
        return Err(tokenization_error(
            entry,
            "tokenizer returned an unexpected batch shape",
        ));
    }
    // Static tokenizers can produce an all-empty batch. ONNX still needs one
    // padding position with a zero attention mask, matching the upstream loader.
    let sequence_length = encoded_length.max(1);
    let batch_size = encodings.len();
    let mut input_ids = Vec::with_capacity(batch_size * sequence_length);
    let mut attention_mask = Vec::with_capacity(batch_size * sequence_length);
    let mut token_type_ids = Vec::with_capacity(batch_size * sequence_length);
    let mut position_ids = Vec::with_capacity(batch_size * sequence_length);
    for encoding in &encodings {
        if encoding.is_empty() {
            input_ids.push(padding_input_id);
            attention_mask.push(0);
            token_type_ids.push(0);
            position_ids.push(0);
            continue;
        }
        input_ids.extend(encoding.get_ids().iter().map(|&value| i64::from(value)));
        attention_mask.extend(
            encoding
                .get_attention_mask()
                .iter()
                .map(|&value| i64::from(value)),
        );
        token_type_ids.extend(
            encoding
                .get_type_ids()
                .iter()
                .map(|&value| i64::from(value)),
        );
        position_ids
            .extend((0..sequence_length).map(|value| i64::try_from(value).unwrap_or(i64::MAX)));
    }
    Ok(PreparedBatch {
        input_ids,
        attention_mask,
        token_type_ids,
        position_ids,
        padding_input_id,
        batch_size,
        sequence_length,
        truncated,
    })
}

fn pool_output(
    shape: &[i64],
    data: &[f32],
    attention_mask: &[i64],
    batch_size: usize,
    sequence_length: usize,
    entry: TransformersConfig,
) -> Result<Vec<Vec<f32>>, ModelError> {
    let expected_batch = i64::try_from(batch_size).unwrap_or(i64::MAX);
    let expected_sequence = i64::try_from(sequence_length).unwrap_or(i64::MAX);
    let expected_dimension = i64::try_from(entry.dimension).unwrap_or(i64::MAX);
    let mut vectors = if shape == [expected_batch, expected_dimension] {
        if data.len() != batch_size * entry.dimension {
            return Err(invalid_tensor(entry, shape));
        }
        data.chunks_exact(entry.dimension)
            .map(<[f32]>::to_vec)
            .collect()
    } else if shape == [expected_batch, expected_sequence, expected_dimension] {
        if data.len() != batch_size * sequence_length * entry.dimension {
            return Err(invalid_tensor(entry, shape));
        }
        let mut vectors = Vec::with_capacity(batch_size);
        for batch_index in 0..batch_size {
            let mut vector = vec![0.0_f64; entry.dimension];
            if entry.pooling == "cls" {
                let offset = batch_index * sequence_length * entry.dimension;
                for (target, &value) in vector
                    .iter_mut()
                    .zip(&data[offset..offset + entry.dimension])
                {
                    *target = f64::from(value);
                }
            } else {
                let mut count = 0_u32;
                for token_index in 0..sequence_length {
                    if attention_mask[batch_index * sequence_length + token_index] == 0 {
                        continue;
                    }
                    count = count.saturating_add(1);
                    let offset = (batch_index * sequence_length + token_index) * entry.dimension;
                    for (target, &value) in vector
                        .iter_mut()
                        .zip(&data[offset..offset + entry.dimension])
                    {
                        *target += f64::from(value);
                    }
                }
                let divisor = f64::from(count.max(1));
                for value in &mut vector {
                    *value /= divisor;
                }
            }
            vectors.push(vector.into_iter().map(narrow_float).collect());
        }
        vectors
    } else {
        return Err(invalid_tensor(entry, shape));
    };
    if entry.normalize {
        for vector in &mut vectors {
            normalize(vector);
        }
    }
    if let Some((vector_index, value_index)) = vectors.iter().enumerate().find_map(|(i, vector)| {
        vector
            .iter()
            .position(|value| !value.is_finite())
            .map(|j| (i, j))
    }) {
        return Err(ModelError::new(
            crate::EngineError::INTERNAL,
            "Transformers returned a non-finite tensor value",
            Some(format!("index={vector_index} offset={value_index}")),
        ));
    }
    Ok(vectors)
}

fn normalize(vector: &mut [f32]) {
    let squared_norm = vector
        .iter()
        .map(|&value| f64::from(value) * f64::from(value))
        .sum::<f64>();
    if squared_norm > 0.0 {
        let inverse = squared_norm.sqrt().recip();
        for value in vector {
            *value = narrow_float(f64::from(*value) * inverse);
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn narrow_float(value: f64) -> f32 {
    value as f32
}

fn invalid_tensor(entry: TransformersConfig, shape: &[i64]) -> ModelError {
    ModelError::new(
        crate::EngineError::INTERNAL,
        "Transformers returned an unexpected tensor",
        Some(format!(
            "expected=batchx{} actual={}",
            entry.dimension,
            shape
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join("x")
        )),
    )
}

fn tokenization_error(entry: TransformersConfig, cause: impl std::fmt::Display) -> ModelError {
    ModelError::new(
        crate::EngineError::INTERNAL,
        "Transformers tokenization failed",
        Some(format!("model={} repo={}", entry.reference, entry.repo)),
    )
    .with_cause(cause)
}

fn check_cancelled(signal: Option<&CancellationToken>) -> Result<(), ModelError> {
    if signal.is_some_and(CancellationToken::is_cancelled) {
        return Err(ModelError::cancelled(
            "Transformers embedding was cancelled",
        ));
    }
    Ok(())
}

fn lock_session(session: &StdMutex<Session>) -> StdMutexGuard<'_, Session> {
    lock_std_mutex(session)
}

fn lock_std_mutex<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn onnx_artifact(entry: TransformersConfig) -> Result<&'static str, ModelError> {
    let mut models = entry.download.artifacts.iter().filter(|artifact| {
        Path::new(artifact.path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("onnx"))
    });
    match (models.next(), models.next()) {
        (Some(artifact), None) => Ok(artifact.path),
        _ => Err(ModelError::storage_failure(
            "Expected exactly one ONNX model artifact",
        )),
    }
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

#[cfg(test)]
mod tests;
