//! Dedicated execution thread for the MLX backend.
//!
//! `mlx_rs::Array` is `!Send`, so every array must be created, used, and
//! evaluated on the thread that owns MLX. [`MlxWorker`] owns exactly such a
//! thread: callers move plain token ids in and receive plain vectors out, and
//! the worker serializes jobs in submission order. It is a single execution
//! thread, not a scheduler: the runtime still owns admission and the execution
//! budget, and concurrent callers queue here rather than forking a second pool.
//!
//! The thread survives a panicking forward pass (`catch_unwind`) so one bad
//! batch cannot poison the cached runtime, and it re-checks the caller's
//! [`CancellationToken`] between sub-batches for cooperative cancellation.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    thread::{self, JoinHandle},
};

use mlx_rs::DeviceType;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::models::spi::ModelError;

/// Padded, mask-complete sub-batch handed to the numerics layer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TokenBatch {
    pub(super) input_ids: Vec<u32>,
    pub(super) attention_mask: Vec<f32>,
    pub(super) batch_size: usize,
    pub(super) sequence_length: usize,
}

/// Padded sequence lengths round up to this multiple so the compiled forward
/// sees a small, repeating set of shapes instead of one per input length.
const SEQUENCE_BUCKET: usize = 128;

impl TokenBatch {
    /// Pads right with token id 0 and derives the attention mask from the real
    /// lengths. Every sequence must be non-empty (the SPI rejects empty input).
    fn pad(sequences: &[Vec<u32>]) -> Self {
        let longest = sequences.iter().map(Vec::len).max().unwrap_or(0).max(1);
        // Bucket the padded length so shapes repeat across calls: the compiled
        // forward keeps one graph per exact (batch, sequence) shape, and an
        // unbucketed length would rebuild the graph on nearly every call.
        // Bucket padding is masked out, so it cannot change the embeddings.
        let sequence_length = longest.div_ceil(SEQUENCE_BUCKET) * SEQUENCE_BUCKET;
        let mut input_ids = Vec::with_capacity(sequences.len() * sequence_length);
        let mut attention_mask = Vec::with_capacity(sequences.len() * sequence_length);
        for sequence in sequences {
            input_ids.extend(sequence.iter().copied());
            input_ids.resize(input_ids.len() + sequence_length - sequence.len(), 0);
            attention_mask.extend(std::iter::repeat_n(1.0_f32, sequence.len()));
            attention_mask.resize(attention_mask.len() + sequence_length - sequence.len(), 0.0);
        }
        Self {
            input_ids,
            attention_mask,
            batch_size: sequences.len(),
            sequence_length,
        }
    }
}

/// Numerics layer evaluated on the worker thread (`F` never leaves it).
pub(super) trait MlxForward: 'static {
    fn forward(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError>;
}

enum Command {
    Prepare {
        signal: Option<CancellationToken>,
        reply: oneshot::Sender<Result<(), ModelError>>,
    },
    Embed {
        ids: Vec<Vec<u32>>,
        signal: Option<CancellationToken>,
        reply: oneshot::Sender<Result<Vec<Vec<f32>>, ModelError>>,
    },
    /// Reports the device the worker actually runs on, for the GPU assertion
    /// in the numerics gate.
    #[cfg(test)]
    Device {
        reply: oneshot::Sender<Result<DeviceType, ModelError>>,
    },
    Shutdown,
}

/// Handle to the model-owning thread.
pub(super) struct MlxWorker {
    commands: mpsc::UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
    /// Fired once the worker thread has created its device and stream. Awaited
    /// by `prepare`, so neither the constructor nor an async caller blocks on
    /// device initialization.
    ready: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    /// Set when readiness timed out: the thread may still be stuck in device
    /// initialization, so `Drop` must not join it.
    abandoned: std::sync::atomic::AtomicBool,
}

/// How long `prepare` waits for the worker to create its device. Test builds
/// use a short window so the timeout branch is exercised deterministically.
#[cfg(test)]
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(not(test))]
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl MlxWorker {
    /// Spawns the worker. `loader` runs on the worker thread on the first
    /// successful [`prepare`](Self::prepare); it is retried on a later prepare
    /// if it fails, so a transient load failure leaves the backend usable.
    /// Test-only constructor: panics if the thread cannot start, which is fine
    /// for tests but not for `prepare`.
    #[cfg(test)]
    pub(super) fn spawn<F>(
        loader: impl Fn(&CancellationToken) -> Result<F, ModelError> + Send + 'static,
        max_batch_size: usize,
    ) -> Self
    where
        F: MlxForward,
    {
        Self::try_spawn(loader, max_batch_size, DeviceType::Cpu).expect("MLX worker thread spawns")
    }

    /// Fallible constructor used by the backend, so a thread-spawn failure
    /// surfaces as a structured error instead of a panic inside `prepare`.
    pub(super) fn try_spawn<F>(
        loader: impl Fn(&CancellationToken) -> Result<F, ModelError> + Send + 'static,
        max_batch_size: usize,
        device: DeviceType,
    ) -> Result<Self, ModelError>
    where
        F: MlxForward,
    {
        Self::try_spawn_impl(loader, max_batch_size, device, false, false)
    }

    /// Test-only constructor that fails device initialization, so the
    /// "device initialization failed" branch has coverage.
    #[cfg(test)]
    pub(super) fn try_spawn_with_failing_device<F>(
        loader: impl Fn(&CancellationToken) -> Result<F, ModelError> + Send + 'static,
        max_batch_size: usize,
    ) -> Result<Self, ModelError>
    where
        F: MlxForward,
    {
        Self::try_spawn_impl(loader, max_batch_size, DeviceType::Cpu, true, false)
    }

    /// Test-only constructor whose device comes up later than `READY_TIMEOUT`,
    /// so the timeout and no-join paths are exercised deterministically.
    #[cfg(test)]
    pub(super) fn try_spawn_with_delayed_device<F>(
        loader: impl Fn(&CancellationToken) -> Result<F, ModelError> + Send + 'static,
        max_batch_size: usize,
    ) -> Result<Self, ModelError>
    where
        F: MlxForward,
    {
        Self::try_spawn_impl(loader, max_batch_size, DeviceType::Cpu, false, true)
    }

    fn try_spawn_impl<F>(
        loader: impl Fn(&CancellationToken) -> Result<F, ModelError> + Send + 'static,
        max_batch_size: usize,
        device: DeviceType,
        fail_device: bool,
        delay_ready: bool,
    ) -> Result<Self, ModelError>
    where
        F: MlxForward,
    {
        let (commands, mut receiver) = mpsc::unbounded_channel();
        // The device and its stream are created inside the thread; readiness is
        // awaited by `prepare`, so nothing blocks here.
        let (ready, device_ready) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("zg-mlx-embed".to_owned())
            .spawn(move || {
                if fail_device {
                    // No readiness signal is sent, so the caller reports a
                    // structured initialization error.
                    return;
                }
                if delay_ready {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                // Pin the model-owning thread to the resolved device, so the
                // arrays and compiled graphs it creates use exactly that device.
                mlx_rs::with_device(mlx_rs::Device::new(device, 0), || {
                    if ready.send(()).is_err() {
                        return;
                    }
                    let mut forward: Option<F> = None;
                    let max_batch_size = max_batch_size.max(1);
                    while let Some(command) = receiver.blocking_recv() {
                        match command {
                            Command::Prepare { signal, reply } => {
                                if forward.is_some() {
                                    let _ = reply.send(Ok(()));
                                    continue;
                                }
                                // The loader runs here, on the array-owning thread,
                                // so weights and compiled graphs never cross a
                                // thread boundary. A failed or cancelled load stays
                                // retryable: `forward` is only replaced on success.
                                match load(&loader, signal.as_ref()) {
                                    Ok(prepared) => {
                                        forward = Some(prepared);
                                        let _ = reply.send(Ok(()));
                                    }
                                    Err(error) => {
                                        let _ = reply.send(Err(error));
                                    }
                                }
                            }
                            Command::Embed { ids, signal, reply } => {
                                let result = match forward.as_mut() {
                                    Some(forward) => embed(
                                        forward,
                                        &ids,
                                        signal.as_ref(),
                                        max_batch_size,
                                        || reply.is_closed(),
                                    ),
                                    None => Err(ModelError::internal(
                                        "MLX embedding model is not prepared",
                                    )),
                                };
                                let _ = reply.send(result);
                            }
                            #[cfg(test)]
                            Command::Device { reply } => {
                                let device = mlx_rs::Stream::thread_local_or_default()
                                    .device()
                                    .and_then(|device| device.get_type())
                                    .map_err(|error| {
                                        ModelError::internal("MLX device query failed")
                                            .with_cause(error)
                                    });
                                let _ = reply.send(device);
                            }
                            Command::Shutdown => break,
                        }
                    }
                });
            })
            .map_err(|error| {
                ModelError::internal("MLX worker thread failed to start").with_cause(error)
            })?;
        Ok(Self {
            commands,
            thread: Some(thread),
            ready: std::sync::Mutex::new(Some(device_ready)),
            abandoned: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Waits once for the worker's device to come up. A timed-out or failed
    /// initialization is a structured error, and the worker is marked
    /// abandoned so dropping the handle never joins a stuck thread.
    async fn await_ready(&self) -> Result<(), ModelError> {
        let receiver = self
            .ready
            .lock()
            .expect("MLX readiness lock is not poisoned")
            .take();
        let Some(receiver) = receiver else {
            return Ok(());
        };
        match tokio::time::timeout(READY_TIMEOUT, receiver).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ModelError::internal(
                "MLX device initialization failed on the worker thread",
            )),
            Err(_) => {
                self.abandoned
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Err(ModelError::internal(
                    "MLX device initialization timed out on the worker thread",
                ))
            }
        }
    }

    pub(super) async fn prepare(
        &self,
        signal: Option<CancellationToken>,
    ) -> Result<(), ModelError> {
        self.await_ready().await?;
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Prepare { signal, reply })?;
        receiver
            .await
            .map_err(|error| ModelError::internal("MLX worker stopped").with_cause(error))?
    }

    pub(super) async fn embed(
        &self,
        ids: Vec<Vec<u32>>,
        signal: Option<CancellationToken>,
    ) -> Result<Vec<Vec<f32>>, ModelError> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Embed { ids, signal, reply })?;
        receiver
            .await
            .map_err(|error| ModelError::internal("MLX worker stopped").with_cause(error))?
    }

    /// Device the worker actually runs on, queried on the owning thread.
    #[cfg(test)]
    pub(super) async fn device_type(&self) -> Result<DeviceType, ModelError> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Device { reply })?;
        receiver
            .await
            .map_err(|error| ModelError::internal("MLX worker stopped").with_cause(error))?
    }

    fn send(&self, command: Command) -> Result<(), ModelError> {
        self.commands
            .send(command)
            .map_err(|_| ModelError::internal("MLX worker has stopped"))
    }
}

impl Drop for MlxWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if self.abandoned.load(std::sync::atomic::Ordering::Relaxed) {
            // Device initialization never completed; joining here could block
            // forever, so the handle is detached instead.
            self.thread.take();
            return;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn load<F>(
    loader: &impl Fn(&CancellationToken) -> Result<F, ModelError>,
    signal: Option<&CancellationToken>,
) -> Result<F, ModelError> {
    check_cancelled(signal)?;
    let signal = signal.cloned().unwrap_or_default();
    let result = catch_unwind(AssertUnwindSafe(|| loader(&signal)))
        .map_err(|panic| panic_error("weight loading", &*panic))?;
    result.and_then(|forward| {
        check_cancelled(Some(&signal))?;
        Ok(forward)
    })
}

fn embed<F: MlxForward>(
    forward: &mut F,
    ids: &[Vec<u32>],
    signal: Option<&CancellationToken>,
    max_batch_size: usize,
    abandoned: impl Fn() -> bool,
) -> Result<Vec<Vec<f32>>, ModelError> {
    let mut vectors = Vec::with_capacity(ids.len());
    for sequences in ids.chunks(max_batch_size) {
        check_cancelled(signal)?;
        // A caller that dropped its future releases the admission permit, so
        // the worker must stop rather than compute a batch nobody receives.
        if abandoned() {
            return Err(ModelError::cancelled(
                "MLX embedding caller stopped waiting for the result",
            ));
        }
        let batch = TokenBatch::pad(sequences);
        let result = catch_unwind(AssertUnwindSafe(|| forward.forward(&batch)))
            .map_err(|panic| panic_error("forward pass", &*panic))?;
        let sub_vectors = result?;
        // The final chunk has no next iteration, so cancellation during the
        // last forward would otherwise return vectors as if it succeeded.
        check_cancelled(signal)?;
        if abandoned() {
            return Err(ModelError::cancelled(
                "MLX embedding caller stopped waiting for the result",
            ));
        }
        if sub_vectors.len() != sequences.len() {
            return Err(ModelError::internal(format!(
                "MLX forward returned {} vectors for {} inputs",
                sub_vectors.len(),
                sequences.len()
            )));
        }
        vectors.extend(sub_vectors);
    }
    Ok(vectors)
}

fn check_cancelled(signal: Option<&CancellationToken>) -> Result<(), ModelError> {
    if signal.is_some_and(CancellationToken::is_cancelled) {
        return Err(ModelError::cancelled("MLX embedding was cancelled"));
    }
    Ok(())
}

fn panic_error(phase: &str, panic: &(dyn std::any::Any + Send)) -> ModelError {
    let message = panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned());
    ModelError::internal(format!("MLX {phase} panicked")).with_cause(message)
}

#[cfg(test)]
mod tests;
