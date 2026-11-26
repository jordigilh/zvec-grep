#![allow(clippy::cast_precision_loss)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{MlxForward, MlxWorker, TokenBatch};
use crate::models::spi::ModelError;

/// Deterministic stand-in for the numerics layer: the first token of each row
/// becomes the row's vector, so replies can be matched back to inputs.
struct EchoForward {
    calls: Arc<std::sync::Mutex<Vec<Vec<u32>>>>,
}

impl EchoForward {
    fn new() -> (Self, Arc<std::sync::Mutex<Vec<Vec<u32>>>>) {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }

    fn first_tokens(batch: &TokenBatch) -> Vec<Vec<f32>> {
        (0..batch.batch_size)
            .map(|row| {
                let offset = row * batch.sequence_length;
                vec![batch.input_ids[offset] as f32]
            })
            .collect()
    }
}

impl MlxForward for EchoForward {
    fn forward(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
        self.calls
            .lock()
            .expect("call log")
            .push(batch.input_ids.clone());
        Ok(Self::first_tokens(batch))
    }
}

/// Cancels the request inside the first sub-batch, after it has been recorded.
struct CancellingForward {
    signal: CancellationToken,
    calls: Arc<std::sync::Mutex<usize>>,
}

impl MlxForward for CancellingForward {
    fn forward(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
        *self.calls.lock().expect("call count") += 1;
        self.signal.cancel();
        Ok(EchoForward::first_tokens(batch))
    }
}

/// Panics when it sees token `13`, otherwise behaves like `EchoForward`.
struct PanickingForward;

impl MlxForward for PanickingForward {
    fn forward(&mut self, batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
        assert!(!batch.input_ids.contains(&13), "injected forward panic");
        Ok(EchoForward::first_tokens(batch))
    }
}

fn spawn_echo(max_batch_size: usize) -> MlxWorker {
    MlxWorker::spawn(|_| Ok(EchoForward::new().0), max_batch_size)
}

#[tokio::test]
async fn prepare_is_idempotent_and_loads_once() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&loads);
    let worker = MlxWorker::spawn(
        move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(EchoForward::new().0)
        },
        2,
    );
    worker.prepare(None).await.expect("first prepare");
    worker.prepare(None).await.expect("second prepare");
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prepare_retries_after_a_structured_load_failure() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&loads);
    let worker = MlxWorker::spawn(
        move |_| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                Err(ModelError::storage_failure("malformed weights"))
            } else {
                Ok(EchoForward::new().0)
            }
        },
        2,
    );
    let error = worker.prepare(None).await.expect_err("first load fails");
    assert_eq!(error.code(), crate::EngineError::STORAGE_FAILURE);
    worker.prepare(None).await.expect("retry succeeds");
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    // The recovered worker actually runs.
    assert_eq!(
        worker.embed(vec![vec![7]], None).await.expect("embed"),
        vec![vec![7.0]]
    );
}

#[tokio::test]
async fn cancelled_prepare_does_not_touch_the_loader() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&loads);
    let worker = MlxWorker::spawn(
        move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(EchoForward::new().0)
        },
        2,
    );
    let signal = CancellationToken::new();
    signal.cancel();
    let error = worker
        .prepare(Some(signal))
        .await
        .expect_err("cancelled prepare");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert_eq!(loads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_during_prepare_reports_cancelled() {
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let worker = MlxWorker::spawn(
        move |signal| {
            started_tx.send(()).expect("signal load start");
            while !signal.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(EchoForward::new().0)
        },
        2,
    );
    let signal = CancellationToken::new();
    let task = tokio::spawn({
        let signal = signal.clone();
        async move { worker.prepare(Some(signal)).await }
    });
    started_rx.recv().await.expect("loader started");
    signal.cancel();
    let error = task.await.expect("join").expect_err("cancelled");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
}

#[tokio::test]
async fn sub_batches_run_in_order_and_reassemble_one_to_one() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let loader_calls = Arc::clone(&calls);
    let worker = MlxWorker::spawn(
        move |_| {
            Ok(EchoForward {
                calls: Arc::clone(&loader_calls),
            })
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");
    let result = worker
        .embed(vec![vec![10], vec![11], vec![12], vec![13], vec![14]], None)
        .await
        .expect("embed");
    assert_eq!(
        result,
        vec![vec![10.0], vec![11.0], vec![12.0], vec![13.0], vec![14.0]]
    );
    // The echo would produce the same output if all five arrived in one call,
    // so assert the batching itself: three calls holding 2, 2 and 1 real tokens.
    let calls = calls.lock().expect("call log");
    assert_eq!(
        calls.len(),
        3,
        "five inputs with batch 2 must be three calls"
    );
    assert_eq!(
        calls.iter().map(|ids| ids[0]).collect::<Vec<_>>(),
        vec![10, 12, 14]
    );
    assert_eq!(
        calls
            .iter()
            .map(|ids| ids.iter().filter(|id| **id != 0).count())
            .collect::<Vec<_>>(),
        vec![2, 2, 1]
    );
}

#[tokio::test]
async fn concurrent_callers_receive_their_own_ordered_replies() {
    let worker = Arc::new(spawn_echo(1));
    worker.prepare(None).await.expect("prepare");
    let mut tasks = Vec::new();
    for value in 0..16_u32 {
        let worker = Arc::clone(&worker);
        tasks.push(tokio::spawn(async move {
            let result = worker
                .embed(vec![vec![value], vec![value + 100]], None)
                .await
                .expect("embed");
            (value, result)
        }));
    }
    for task in tasks {
        let (value, result) = task.await.expect("join");
        assert_eq!(result, vec![vec![value as f32], vec![value as f32 + 100.0]]);
    }
}

#[tokio::test]
async fn cancellation_is_checked_between_sub_batches() {
    let signal = CancellationToken::new();
    let calls = Arc::new(std::sync::Mutex::new(0_usize));
    let signal_for_loader = signal.clone();
    let calls_for_forward = Arc::clone(&calls);
    let worker = MlxWorker::spawn(
        move |_| {
            Ok(CancellingForward {
                signal: signal_for_loader.clone(),
                calls: Arc::clone(&calls_for_forward),
            })
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");
    let error = worker
        .embed(
            vec![vec![1], vec![2], vec![3], vec![4], vec![5]],
            Some(signal),
        )
        .await
        .expect_err("cancelled between sub-batches");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert_eq!(*calls.lock().expect("call count"), 1);
}

#[tokio::test]
async fn a_panicking_forward_does_not_kill_the_worker() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&loads);
    let worker = MlxWorker::spawn(
        move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(PanickingForward)
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");
    let error = worker
        .embed(vec![vec![13]], None)
        .await
        .expect_err("panic is isolated");
    assert_eq!(error.code(), crate::EngineError::INTERNAL);
    // Same worker, same loaded forward, next job succeeds.
    assert_eq!(
        worker.embed(vec![vec![3]], None).await.expect("embed"),
        vec![vec![3.0]]
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn embed_before_prepare_is_a_structured_error() {
    let worker = spawn_echo(2);
    let error = worker
        .embed(vec![vec![1]], None)
        .await
        .expect_err("not prepared");
    assert_eq!(error.code(), crate::EngineError::INTERNAL);
}

#[tokio::test]
async fn cancellation_during_the_only_sub_batch_is_reported() {
    let signal = CancellationToken::new();
    let calls = Arc::new(std::sync::Mutex::new(0_usize));
    let signal_for_loader = signal.clone();
    let calls_for_forward = Arc::clone(&calls);
    // The whole request fits in one chunk, so only the post-forward check can
    // observe the cancellation the forward itself raises.
    let worker = MlxWorker::spawn(
        move |_| {
            Ok(CancellingForward {
                signal: signal_for_loader.clone(),
                calls: Arc::clone(&calls_for_forward),
            })
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");
    let error = worker
        .embed(vec![vec![1], vec![2]], Some(signal))
        .await
        .expect_err("cancelled during the final forward");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert_eq!(*calls.lock().expect("call count"), 1);
}

#[test]
fn a_closed_reply_before_a_chunk_skips_the_forward() {
    let (mut forward, calls) = EchoForward::new();
    let error = super::embed(
        &mut forward,
        &[vec![1], vec![2]],
        None,
        1,
        // The caller is already gone when the command is dequeued.
        || true,
    )
    .expect_err("closed reply");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert!(
        calls.lock().expect("call log").is_empty(),
        "no batch may run for a caller that stopped waiting"
    );
}

#[test]
fn a_reply_that_closes_during_a_forward_stops_the_next_chunk() {
    let (mut forward, calls) = EchoForward::new();
    let mut checks = 0;
    let error = super::embed(&mut forward, &[vec![1], vec![2]], None, 1, || {
        checks += 1;
        // Open before the first chunk, closed once its forward returns.
        checks > 1
    })
    .expect_err("closed during the forward");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert_eq!(
        calls.lock().expect("call log").len(),
        1,
        "the second chunk must not run after the caller disappeared"
    );
}

#[test]
fn padding_derives_the_attention_mask_from_real_lengths() {
    let batch = TokenBatch::pad(&[vec![1, 2, 3], vec![4]]);
    assert_eq!(batch.batch_size, 2);
    assert_eq!(batch.sequence_length, 128, "padded to the first bucket");
    // Row 0 keeps its three tokens, row 1 starts at the second row offset.
    assert_eq!(&batch.input_ids[..3], &[1, 2, 3]);
    assert_eq!(batch.input_ids[3], 0, "row 0 padding");
    assert_eq!(batch.input_ids[128], 4);
    assert_eq!(batch.input_ids[129], 0, "row 1 padding");
    assert_eq!(&batch.attention_mask[..3], &[1.0, 1.0, 1.0]);
    assert!(batch.attention_mask[3].abs() <= f32::EPSILON);
    assert!((batch.attention_mask[128] - 1.0).abs() <= f32::EPSILON);
    assert!(batch.attention_mask[129].abs() <= f32::EPSILON);
}

#[test]
fn padding_rounds_up_to_bucket_boundaries_so_compiled_shapes_repeat() {
    let batch = super::TokenBatch::pad(&[vec![1, 2, 3], vec![4; 129]]);
    assert_eq!(batch.batch_size, 2);
    assert_eq!(
        batch.sequence_length, 256,
        "129 tokens round up to two buckets"
    );
    assert_eq!(&batch.attention_mask[..3], &[1.0, 1.0, 1.0]);
    assert!(
        batch.attention_mask[3].abs() <= f32::EPSILON,
        "bucket padding stays masked out"
    );

    let short = super::TokenBatch::pad(&[vec![1, 2, 3]]);
    assert_eq!(short.sequence_length, 128, "short inputs share one bucket");
}

#[tokio::test]
async fn a_device_initialization_failure_is_a_structured_error() {
    let worker = MlxWorker::try_spawn_with_failing_device(|_| Ok(EchoForward::new().0), 2)
        .expect("spawn succeeds; failure surfaces on prepare");
    let error = worker.prepare(None).await.expect_err("device failure");
    assert_eq!(error.code(), crate::EngineError::INTERNAL);
}

#[tokio::test]
async fn a_late_device_initialization_times_out_without_joining() {
    let worker = MlxWorker::try_spawn_with_delayed_device(|_| Ok(EchoForward::new().0), 2)
        .expect("spawn succeeds");
    let error = worker.prepare(None).await.expect_err("readiness times out");
    assert_eq!(error.code(), crate::EngineError::INTERNAL);
    // Dropping must return even though the worker thread is still starting up.
    let dropped_at = std::time::Instant::now();
    drop(worker);
    assert!(
        dropped_at.elapsed() < Duration::from_millis(150),
        "drop joined the still-running device start"
    );
}

#[tokio::test]
async fn cancellation_during_device_startup_returns_cancelled() {
    let worker = MlxWorker::try_spawn_with_delayed_device(|_| Ok(EchoForward::new().0), 2)
        .expect("spawn succeeds");
    let signal = CancellationToken::new();
    let prepare = worker.prepare(Some(signal.clone()));
    tokio::pin!(prepare);
    tokio::select! {
        result = &mut prepare => panic!("readiness resolved before the device started: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    signal.cancel();
    let error = prepare.await.expect_err("cancelled readiness wait");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
}

#[tokio::test]
async fn dropping_the_prepare_future_during_device_start_returns_quickly() {
    let worker = MlxWorker::try_spawn_with_delayed_device(|_| Ok(EchoForward::new().0), 2)
        .expect("spawn succeeds");
    {
        let prepare = worker.prepare(None);
        tokio::pin!(prepare);
        tokio::select! {
            result = &mut prepare => panic!("readiness resolved before the device started: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    let dropped_at = std::time::Instant::now();
    drop(worker);
    assert!(
        dropped_at.elapsed() < Duration::from_millis(150),
        "drop joined the device start whose waiter was already gone"
    );
}

#[tokio::test]
async fn the_worker_reports_the_device_it_was_pinned_to() {
    let worker = MlxWorker::try_spawn(|_| Ok(EchoForward::new().0), 2, mlx_rs::DeviceType::Cpu)
        .expect("worker");
    worker.prepare(None).await.expect("prepare");
    assert!(matches!(
        worker.device_type().await.expect("device"),
        mlx_rs::DeviceType::Cpu
    ));
}

#[tokio::test]
async fn already_cancelled_embed_returns_cancelled_without_running_the_forward() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let loader_calls = Arc::clone(&calls);
    let worker = MlxWorker::spawn(
        move |_| {
            Ok(EchoForward {
                calls: Arc::clone(&loader_calls),
            })
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");

    let signal = CancellationToken::new();
    signal.cancel();
    let error = worker
        .embed(vec![vec![1], vec![2]], Some(signal))
        .await
        .expect_err("a token cancelled before submit must not run the forward");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    assert!(
        calls.lock().expect("calls").is_empty(),
        "the forward must not be called for an already-cancelled request"
    );
}

#[tokio::test]
async fn embed_still_works_after_a_cancelled_prepare() {
    let worker = spawn_echo(2);
    let signal = CancellationToken::new();
    signal.cancel();
    let error = worker
        .prepare(Some(signal))
        .await
        .expect_err("cancelled prepare");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);

    worker
        .prepare(None)
        .await
        .expect("retry after cancellation");
    let vectors = worker.embed(vec![vec![7]], None).await.expect("embed");
    assert_eq!(vectors, vec![vec![7.0]]);
}

/// Waits for a detached worker to finish, so ownership assertions do not race
/// with the thread's exit.
fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    condition()
}

#[tokio::test]
async fn dropping_the_worker_frees_the_model_on_its_own_thread() {
    struct DropTracking {
        dropped: Arc<AtomicUsize>,
        created_on: std::thread::ThreadId,
        observed: Arc<std::sync::Mutex<Option<(std::thread::ThreadId, std::thread::ThreadId)>>>,
    }

    impl super::MlxForward for DropTracking {
        fn forward(&mut self, _batch: &TokenBatch) -> Result<Vec<Vec<f32>>, ModelError> {
            Ok(Vec::new())
        }
    }

    impl Drop for DropTracking {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
            *self.observed.lock().expect("drop thread") =
                Some((self.created_on, std::thread::current().id()));
        }
    }

    let dropped = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(std::sync::Mutex::new(None));
    let loader_dropped = Arc::clone(&dropped);
    let loader_observed = Arc::clone(&observed);
    let caller = std::thread::current().id();
    let worker = MlxWorker::spawn(
        move |_| {
            Ok(DropTracking {
                dropped: Arc::clone(&loader_dropped),
                created_on: std::thread::current().id(),
                observed: Arc::clone(&loader_observed),
            })
        },
        2,
    );
    worker.prepare(None).await.expect("prepare");
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(worker);
    assert!(
        wait_for(|| observed.lock().expect("drop thread").is_some()),
        "the worker thread never finished teardown"
    );
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "the model is dropped with the worker thread, not left behind"
    );
    let (created_on, dropped_on) = observed
        .lock()
        .expect("drop thread")
        .expect("model dropped");
    assert_eq!(
        created_on, dropped_on,
        "the model must be dropped on the thread that created it"
    );
    assert_ne!(dropped_on, caller, "the caller must not drop the model");
}
