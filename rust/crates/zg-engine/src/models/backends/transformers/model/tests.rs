use super::*;

#[test]
fn maps_devices_to_native_rust_execution_providers() {
    assert_eq!(
        resolve_execution_provider(None),
        TransformersExecutionProvider::Cpu
    );
    assert_eq!(
        resolve_execution_provider(Some(Device::Cpu)),
        TransformersExecutionProvider::Cpu
    );
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            resolve_execution_provider(Some(Device::Metal)),
            TransformersExecutionProvider::Cpu
        );
        assert_eq!(
            resolve_execution_provider(Some(Device::Auto)),
            TransformersExecutionProvider::Cpu
        );
    }
    // Providers this build cannot initialize never resolve: the session would
    // only fall back to CPU at load time while the recorded backend identity
    // still named the unavailable provider.
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        resolve_execution_provider(Some(Device::Metal)),
        TransformersExecutionProvider::Cpu
    );
    assert_provider_resolution(
        Some(Device::Vulkan),
        cfg!(feature = "vulkan"),
        TransformersExecutionProvider::WebGpu,
    );
    assert_provider_resolution(
        Some(Device::Cuda),
        cfg!(feature = "cuda"),
        TransformersExecutionProvider::Cuda,
    );
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    assert_provider_resolution(
        Some(Device::Auto),
        cfg!(feature = "cuda"),
        TransformersExecutionProvider::Cuda,
    );
    #[cfg(target_os = "windows")]
    assert_eq!(
        resolve_execution_provider(Some(Device::Auto)),
        TransformersExecutionProvider::DirectMl
    );
}

fn assert_provider_resolution(
    device: Option<Device>,
    available: bool,
    provider: TransformersExecutionProvider,
) {
    let resolved = resolve_execution_provider(device);
    if available {
        assert_eq!(resolved, provider);
    } else {
        assert_eq!(resolved, TransformersExecutionProvider::Cpu);
    }
}

#[test]
fn backend_identity_names_the_provider_that_produces_vectors() {
    // A pool that fell back to CPU reports CPU, so its generations record
    // onnx-cpu instead of the accelerated provider its device requested.
    assert_eq!(
        transformers_backend_identity(TransformersExecutionProvider::Cpu, "q8").backend,
        "onnx-cpu"
    );
    assert_eq!(
        transformers_backend_identity(TransformersExecutionProvider::CoreMl, "q8").backend,
        "onnx-coreml"
    );
    assert_eq!(
        transformers_backend_identity(TransformersExecutionProvider::Cuda, "q4").dtype,
        "q4"
    );
    // Before load, the model reports the feature-aware resolution its
    // constructor recorded, and the same dtype as its pinned artifact.
    let model = TransformersEmbeddingModel::new(
        entry("mean", true),
        crate::domain::model::ModelConfig::default(),
        crate::models::runtime::ModelComputeRuntime::shared(),
    );
    let identity = model.backend_identity().expect("identity");
    assert_eq!(
        identity.backend,
        format!("onnx-{}", resolve_execution_provider(None).name())
    );
    assert_eq!(identity.dtype, "q8");
}

#[test]
fn coreml_uses_one_physical_session_for_all_user_concurrency() {
    assert_eq!(
        physical_session_limit(TransformersExecutionProvider::CoreMl, 1),
        1
    );
    assert_eq!(
        physical_session_limit(TransformersExecutionProvider::CoreMl, 4),
        1
    );
    assert_eq!(
        physical_session_limit(TransformersExecutionProvider::Cpu, 4),
        4
    );
}

#[test]
fn coreml_batch_merge_repads_rows_and_preserves_indexes() {
    let first = PreparedBatch {
        input_ids: vec![101, 102],
        attention_mask: vec![1, 1],
        token_type_ids: vec![0, 0],
        position_ids: vec![0, 1],
        padding_input_id: 0,
        batch_size: 1,
        sequence_length: 2,
        truncated: vec![0],
    };
    let second = PreparedBatch {
        input_ids: vec![201, 202, 203, 301, 302, 0],
        attention_mask: vec![1, 1, 1, 1, 1, 0],
        token_type_ids: vec![0; 6],
        position_ids: vec![0, 1, 2, 0, 1, 2],
        padding_input_id: 0,
        batch_size: 2,
        sequence_length: 3,
        truncated: vec![1],
    };
    let merged = merge_prepared_batches(&[&first, &second]).expect("merged batch");
    assert_eq!(merged.batch_size, 3);
    assert_eq!(merged.sequence_length, 3);
    assert_eq!(merged.input_ids, [101, 102, 0, 201, 202, 203, 301, 302, 0]);
    assert_eq!(merged.attention_mask, [1, 1, 0, 1, 1, 1, 1, 1, 0]);
    assert_eq!(merged.truncated, [0, 2]);
}
use crate::domain::model::Metric;

fn entry(pooling: &'static str, normalize: bool) -> TransformersConfig {
    let download =
        crate::models::catalog::get_embedding_model_catalog_entry("local/multilingual-e5-small")
            .and_then(crate::models::catalog::EmbeddingCatalogEntry::transformers_config)
            .expect("fixture download metadata")
            .download;
    TransformersConfig {
        reference: "local/test-transformer",
        provider: "local",
        model: "test-transformer",
        repo: "test/model-ONNX",
        revision: "0123456789abcdef",
        download,
        dtype: "q8",
        add_special_tokens: true,
        dimension: 3,
        metric: Metric::Cosine,
        pooling,
        normalize,
        query_prefix: Some("query: "),
        document_prefix: Some("passage: "),
        max_input_tokens: 2,
        max_batch_size: 4,
    }
}

fn tokenizer_fixture(unknown_token: &str) -> Tokenizer {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "type": "WordPiece",
        "unk_token": unknown_token,
        "continuing_subword_prefix": "##",
        "max_input_chars_per_word": 100,
        "vocab": {"[PAD]": 0, "[UNK]": 1, "[CLS]": 2, "[SEP]": 3, "hello": 4, "world": 5}
    }))
    .expect("fixture model JSON");
    let model = tokenizers::models::wordpiece::WordPiece::from_bytes(bytes)
        .expect("fixture WordPiece model");
    let mut tokenizer = Tokenizer::new(model);
    tokenizer.with_pre_tokenizer(Some(
        tokenizers::pre_tokenizers::whitespace::WhitespaceSplit,
    ));
    tokenizer.with_post_processor(Some(tokenizers::processors::bert::BertProcessing::new(
        ("[SEP]".to_owned(), 3),
        ("[CLS]".to_owned(), 2),
    )));
    tokenizer
}

fn tokenization_entry() -> TransformersConfig {
    TransformersConfig {
        max_input_tokens: 4,
        ..entry("mean", true)
    }
}

#[test]
fn shared_tokenizers_preserve_truncation_boundaries_and_batch_padding() {
    let entry = tokenization_entry();
    let tokenizers =
        prepare_tokenizers(tokenizer_fixture("[UNK]"), entry).expect("configured tokenizers");
    let prepared = prepare_batch(
        &tokenizers,
        &[
            String::new(),
            "hello".to_owned(),
            "hello world".to_owned(),
            "hello world hello".to_owned(),
            "unknown".to_owned(),
        ],
        entry,
    )
    .expect("mixed length inputs");
    assert_eq!(prepared.batch_size, 5);
    assert_eq!(prepared.sequence_length, 4);
    assert_eq!(
        prepared.input_ids,
        [2, 3, 0, 0, 2, 4, 3, 0, 2, 4, 5, 3, 2, 4, 5, 3, 2, 1, 3, 0]
    );
    assert_eq!(
        prepared.attention_mask,
        [1, 1, 0, 0, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0]
    );
    assert_eq!(prepared.token_type_ids, vec![0; 20]);
    assert_eq!(prepared.position_ids, [0, 1, 2, 3].repeat(5));
    assert_eq!(prepared.padding_input_id, 0);
    assert_eq!(prepared.truncated, [3]);

    let next = prepare_batch(&tokenizers, &["hello".to_owned()], entry)
        .expect("reuse with a shorter batch");
    assert_eq!(next.input_ids, [2, 4, 3]);
    assert_eq!(next.attention_mask, [1, 1, 1]);
    assert_eq!(next.truncated, Vec::<usize>::new());
}

#[test]
fn concurrent_requests_share_tokenizers_without_changing_configuration() {
    let entry = tokenization_entry();
    let tokenizers =
        prepare_tokenizers(tokenizer_fixture("[UNK]"), entry).expect("configured tokenizers");
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|index| {
                let tokenizers = &tokenizers;
                let barrier = &barrier;
                scope.spawn(move || {
                    let long = index % 2 == 0;
                    let text = if long { "hello world hello" } else { "world" };
                    barrier.wait();
                    for _ in 0..8 {
                        let result = prepare_batch(tokenizers, &[text.to_owned()], entry)
                            .expect("concurrent tokenization");
                        if long {
                            assert_eq!(result.input_ids, [2, 4, 5, 3]);
                            assert_eq!(result.truncated, [0]);
                        } else {
                            assert_eq!(result.input_ids, [2, 5, 3]);
                            assert_eq!(result.truncated, Vec::<usize>::new());
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("tokenization worker");
        }
    });
    assert!(tokenizers.probe.get_padding().is_none());
    assert_eq!(
        tokenizers
            .probe
            .get_truncation()
            .expect("probe limit")
            .max_length,
        5
    );
    assert_eq!(
        tokenizers
            .inputs
            .get_truncation()
            .expect("input limit")
            .max_length,
        4
    );
    assert!(matches!(
        tokenizers
            .inputs
            .get_padding()
            .expect("input padding")
            .strategy,
        PaddingStrategy::BatchLongest
    ));
}

#[test]
fn tokenization_failure_does_not_poison_shared_tokenizers() {
    let entry = tokenization_entry();
    let tokenizers =
        prepare_tokenizers(tokenizer_fixture("[MISSING]"), entry).expect("configured tokenizers");
    let error = prepare_batch(&tokenizers, &["unknown".to_owned()], entry)
        .err()
        .expect("unknown token missing from vocabulary");
    assert_eq!(error.code(), crate::EngineError::INTERNAL);
    assert!(
        error
            .to_string()
            .contains("Transformers tokenization failed")
    );
    let recovered = prepare_batch(&tokenizers, &["hello".to_owned()], entry)
        .expect("known tokens remain usable after failure");
    assert_eq!(recovered.input_ids, [2, 4, 3]);
    assert_eq!(recovered.truncated, Vec::<usize>::new());
}

#[test]
fn mean_pooling_and_normalization_match_transformers_pipeline() {
    let values = [
        1.0, 0.0, 0.0, // first token
        0.0, 1.0, 0.0, // second token
        9.0, 9.0, 9.0, // padding
    ];
    let vectors =
        pool_output(&[1, 3, 3], &values, &[1, 1, 0], 1, 3, entry("mean", true)).expect("pooling");
    assert!((vectors[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!((vectors[0][1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!(vectors[0][2].abs() < f32::EPSILON);
}

#[test]
fn cls_pooling_and_matrix_output_are_supported() {
    let cls = pool_output(
        &[1, 2, 3],
        &[1.0, 2.0, 3.0, 8.0, 8.0, 8.0],
        &[1, 1],
        1,
        2,
        entry("cls", false),
    )
    .expect("CLS pooling");
    assert_eq!(cls, [[1.0, 2.0, 3.0]]);
    let matrix = pool_output(&[1, 3], &[1.0, 2.0, 3.0], &[1], 1, 1, entry("mean", false))
        .expect("matrix output");
    assert_eq!(matrix, [[1.0, 2.0, 3.0]]);
}

#[test]
fn prefixes_and_catalog_info_match_main() {
    let model = TransformersEmbeddingModel::new(
        entry("mean", true),
        ModelConfig::default(),
        crate::models::runtime::ModelComputeRuntime::shared(),
    );
    assert_eq!(model.info().max_input_tokens, Some(2));
    assert_eq!(
        onnx_artifact(entry("mean", true)).expect("q8"),
        "onnx/model_quantized.onnx"
    );
}

#[test]
fn static_batches_omit_special_tokens_and_handle_empty_and_truncated_text() {
    let entry = TransformersConfig {
        add_special_tokens: false,
        ..tokenization_entry()
    };
    let tokenizers = prepare_tokenizers(tokenizer_fixture("[UNK]"), entry).expect("tokenizers");
    let batch = prepare_batch(
        &tokenizers,
        &[
            String::new(),
            "hello world".to_owned(),
            "hello world hello world hello".to_owned(),
        ],
        entry,
    )
    .expect("batch");
    assert_eq!(batch.input_ids, [0, 0, 0, 0, 4, 5, 0, 0, 4, 5, 4, 5]);
    assert_eq!(batch.attention_mask, [0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1]);
    assert_eq!(batch.truncated, [2]);
    let empty =
        prepare_batch(&tokenizers, &[String::new(), String::new()], entry).expect("empty text");
    assert_eq!(empty.sequence_length, 1);
    assert_eq!(empty.input_ids, [0, 0]);
    assert_eq!(empty.attention_mask, [0, 0]);
}

#[test]
fn normalizes_pooled_sentence_vectors_and_rejects_wrong_dimensions() {
    let mut entry = entry("mean", true);
    entry.dimension = 256;
    let mut data = vec![0.0; 256];
    data[0] = 3.0;
    data[1] = 4.0;
    let vectors = pool_output(&[1, 256], &data, &[1], 1, 1, entry).expect("pooled output");
    assert_eq!(&vectors[0][..2], &[0.6, 0.8]);
    assert!(pool_output(&[1, 128], &data, &[1], 1, 1, entry).is_err());
}

#[tokio::test]
async fn cancelled_initial_load_preserves_cancellation_code() {
    let cache = tempfile::tempdir().expect("model cache");
    let model = TransformersEmbeddingModel::new(
        entry("mean", true),
        ModelConfig {
            cache_dir: Some(cache.path().to_owned()),
            ..ModelConfig::default()
        },
        crate::models::runtime::ModelComputeRuntime::shared(),
    );
    let signal = CancellationToken::new();
    signal.cancel();
    let error = model
        .embed(
            &[vec![Content::Text("cancel before loading".to_owned())]],
            EmbeddingOptions {
                signal: Some(signal),
                ..EmbeddingOptions::default()
            },
        )
        .await
        .expect_err("cancelled load");

    assert_eq!(error.code(), crate::EngineError::CANCELLED);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[ignore = "requires ZVEC_GREP_TEST_MODEL_CACHE with a pinned ONNX model"]
async fn cached_minilm_runs_real_onnx_inference() {
    let cache = env::var_os("ZVEC_GREP_TEST_MODEL_CACHE")
        .map(PathBuf::from)
        .expect("ZVEC_GREP_TEST_MODEL_CACHE must point at the model cache");
    let entry = crate::models::catalog::get_embedding_model_catalog_entry("local/all-minilm-l6-v2")
        .and_then(crate::models::catalog::EmbeddingCatalogEntry::transformers_config)
        .expect("catalog entry");
    let model = TransformersEmbeddingModel::new(
        entry,
        ModelConfig {
            cache_dir: Some(cache),
            device: Some(Device::Cpu),
            ..ModelConfig::default()
        },
        crate::models::runtime::ModelComputeRuntime::shared(),
    );
    let loaded = model
        .ensure_loaded(None, None, 1)
        .await
        .expect("download and load cached ONNX model");
    let prepared = prepare_batch(
        &loaded.tokenizers,
        &[
            "find authentication middleware".to_owned(),
            "parse a configuration file".to_owned(),
        ],
        entry,
    )
    .expect("tokenize main oracle inputs");
    assert_eq!(
        prepared.input_ids,
        [
            101, 2424, 27280, 2690, 8059, 102, 0, 101, 11968, 3366, 1037, 9563, 5371, 102,
        ]
    );
    assert_eq!(
        prepared.attention_mask,
        [1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1]
    );
    assert!(prepared.token_type_ids.iter().all(|&value| value == 0));
    let result = model
        .embed(
            &[
                vec![Content::Text("find authentication middleware".to_owned())],
                vec![Content::Text("parse a configuration file".to_owned())],
            ],
            EmbeddingOptions::default(),
        )
        .await
        .expect("real ONNX embedding");
    assert_eq!(result.vectors.len(), 2);
    assert!(result.vectors.iter().all(|vector| vector.len() == 384));
    let main_first_values = [
        [
            -0.082_118_884,
            0.026_001_254,
            -0.016_771_38,
            -0.107_571_7,
            0.079_822_97,
            -0.018_354_345,
            0.035_343_368,
            0.006_443_004_6,
            -0.010_101_517,
            0.013_123_883,
            0.013_252_53,
            -0.035_912_04,
        ],
        [
            0.027_389_433,
            0.040_037_83,
            -0.059_505_902,
            -0.038_954_25,
            -0.012_910_471,
            -0.016_948_676,
            0.027_840_037,
            0.076_388_21,
            -0.106_726_564,
            -0.027_350_506,
            0.054_113_51,
            0.034_919_977,
        ],
    ];
    for (index, vector) in result.vectors.iter().enumerate() {
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
        for (offset, (&actual, &expected)) in
            vector.iter().zip(&main_first_values[index]).enumerate()
        {
            assert!(
                (actual - expected).abs() < 5e-3,
                "vector={index} offset={offset} actual={actual} expected={expected}"
            );
        }
    }
    assert!(result.truncated.is_empty());

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let error = model
        .embed(
            &[vec![Content::Text("cancel after loading".to_owned())]],
            EmbeddingOptions {
                signal: Some(cancelled),
                ..EmbeddingOptions::default()
            },
        )
        .await
        .expect_err("cancelled inference on a loaded model");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    let recovered = model
        .embed(
            &[
                vec![Content::Text("find authentication middleware".to_owned())],
                vec![Content::Text("parse a configuration file".to_owned())],
            ],
            EmbeddingOptions::default(),
        )
        .await
        .expect("shared tokenizers remain usable after cancellation");
    for (&actual, &expected) in recovered.vectors[0].iter().zip(&result.vectors[0]) {
        assert!((actual - expected).abs() < 1e-5);
    }

    let cores = available_cores();

    // Overlapping leases queue on the aggregate thread budget: while one
    // lease holds the machine-wide session, a claim that cannot fit the
    // remaining budget waits until it releases. The holder parks on a condvar
    // so its budget stays claimed deterministically; the waiter's progress is
    // counted with an atomic, never a clock. This runs before the pooled
    // run(2) phase below so the pool still holds its initial all-cores
    // session, making the waiter's block a budget block rather than a slot
    // availability block: it creates its own session, which fits only once
    // the holder releases.
    {
        let holder_entered = Arc::new((StdMutex::new(false), Condvar::new()));
        let release_holder = Arc::new((StdMutex::new(false), Condvar::new()));
        let holder_entered_for_closure = Arc::clone(&holder_entered);
        let release_holder_for_closure = Arc::clone(&release_holder);
        let holder_loaded = Arc::clone(&loaded);
        let holder = thread::spawn(move || {
            holder_loaded
                .sessions
                .run(1, move |_session| -> Result<(), ModelError> {
                    let (lock, cvar) = &*holder_entered_for_closure;
                    *lock_std_mutex(lock) = true;
                    cvar.notify_one();
                    let (release_lock, release_cvar) = &*release_holder_for_closure;
                    let mut released = lock_std_mutex(release_lock);
                    while !*released {
                        released = release_cvar
                            .wait(released)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    Ok(())
                })
                .expect("holder lease");
        });
        // The claim happens before the closure runs, so once the holder is
        // inside its closure the aggregate budget holds the whole machine.
        {
            let (lock, cvar) = &*holder_entered;
            let mut entered = lock_std_mutex(lock);
            while !*entered {
                entered = cvar.wait(entered).unwrap_or_else(PoisonError::into_inner);
            }
        }
        assert_eq!(
            lock_std_mutex(&loaded.sessions.state).thread_budget.active,
            cores
        );
        let waiter_ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let waiter_loaded = Arc::clone(&loaded);
        let waiter_ran_for_closure = Arc::clone(&waiter_ran);
        // run(2) gives the waiter a cores/2-thread session budget, which
        // cannot fit alongside the holder's machine-wide claim on any host,
        // and caps its session quota at two so the later barrier phase's
        // session count stays deterministic. Note the quota behavior this
        // exposes: a budget-blocked caller fills its session quota before
        // parking — bounded by its execution limit, and the sessions are
        // shared with later callers.
        let waiter = thread::spawn(move || {
            waiter_loaded
                .sessions
                .run(2, move |_session| -> Result<(), ModelError> {
                    waiter_ran_for_closure.fetch_add(1, Ordering::Release);
                    Ok(())
                })
                .expect("waiter lease");
        });
        // The waiter needs at least one more intra-op thread than remains:
        // under the pool lock, where the claim itself runs, it does not fit,
        // and it has not run.
        assert!(
            !lock_std_mutex(&loaded.sessions.state)
                .thread_budget
                .fits(Some(1))
        );
        assert_eq!(waiter_ran.load(Ordering::Acquire), 0);
        // Synchronize on the pool wait path before releasing the holder: the
        // waiting counter is incremented under the pool lock immediately
        // before `Condvar::wait`, so observing it proves the waiter is parked
        // on the budget rather than merely not scheduled yet. Yielding until
        // the state is reached is a condition wait, not a timing assertion —
        // if the queueing were broken the test would hang, not flake.
        while lock_std_mutex(&loaded.sessions.state).waiting == 0 {
            std::thread::yield_now();
        }
        assert_eq!(waiter_ran.load(Ordering::Acquire), 0);
        // Release the holder; the waiter must now proceed.
        {
            let (lock, cvar) = &*release_holder;
            *lock_std_mutex(lock) = true;
            cvar.notify_one();
        }
        holder.join().expect("holder thread");
        waiter.join().expect("waiter thread");
        assert_eq!(waiter_ran.load(Ordering::Acquire), 1);
        assert_eq!(
            lock_std_mutex(&loaded.sessions.state).thread_budget.active,
            0
        );
        assert_eq!(lock_std_mutex(&loaded.sessions.state).waiting, 0);
    }

    // Two concurrent leases sharing the pool. On a single-core host each CPU
    // session needs the entire thread budget, so the second concurrent claim
    // could never start and a two-party barrier would deadlock; exercise the
    // pool sequentially there instead. The holder/waiter scenario above runs
    // on every host: a queued claim that cannot fit the budget is exactly
    // what single-core saturation produces.
    if cores >= 2 {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let first_loaded = Arc::clone(&loaded);
        let first_barrier = Arc::clone(&barrier);
        let first = model.compute_runtime.run(move || {
            first_loaded.sessions.run(2, |_session| {
                first_barrier.wait();
                Ok(())
            })
        });
        let second_loaded = Arc::clone(&loaded);
        let second = model.compute_runtime.run(move || {
            second_loaded.sessions.run(2, |_session| {
                barrier.wait();
                Ok(())
            })
        });
        let (first, second) = tokio::join!(first, second);
        first
            .expect("first compute task")
            .expect("first pooled session");
        second
            .expect("second compute task")
            .expect("second pooled session");
        assert_eq!(lock_std_mutex(&loaded.sessions.state).sessions.len(), 2);
    } else {
        loaded
            .sessions
            .run(2, |_session| Ok::<(), ModelError>(()))
            .expect("first sequential lease");
        loaded
            .sessions
            .run(2, |_session| Ok::<(), ModelError>(()))
            .expect("second sequential lease");
        assert!(!lock_std_mutex(&loaded.sessions.state).sessions.is_empty());
    }

    // A panicking inference closure must not leak the slot or the aggregate
    // CPU thread budget: the RAII lease releases both during unwind, so the
    // pool still serves work afterwards.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = loaded
            .sessions
            .run(1, |_session| -> Result<(), ModelError> {
                panic!("inference panic")
            });
    }));
    std::panic::set_hook(previous_hook);
    assert!(panicked.is_err());
    {
        let state = lock_std_mutex(&loaded.sessions.state);
        assert_eq!(state.thread_budget.active, 0);
        assert!(
            state
                .sessions
                .iter()
                .all(|slot| !slot.busy.load(Ordering::Acquire))
        );
    }
    loaded
        .sessions
        .run(1, |_session| Ok::<(), ModelError>(()))
        .expect("pool serves work after a panicking inference");
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires ZVEC_GREP_TEST_MODEL_CACHE with a pinned ONNX model"]
async fn cached_minilm_routes_metal_to_cpu_with_warning() {
    let cache = env::var_os("ZVEC_GREP_TEST_MODEL_CACHE")
        .map(PathBuf::from)
        .expect("ZVEC_GREP_TEST_MODEL_CACHE must point at the model cache");
    let entry = crate::models::catalog::get_embedding_model_catalog_entry("local/all-minilm-l6-v2")
        .and_then(crate::models::catalog::EmbeddingCatalogEntry::transformers_config)
        .expect("catalog entry");
    let warnings = Arc::new(StdMutex::new(Vec::new()));
    let captured = Arc::clone(&warnings);
    let model = TransformersEmbeddingModel::new(
        entry,
        ModelConfig {
            cache_dir: Some(cache),
            device: Some(Device::Metal),
            ..ModelConfig::default()
        },
        crate::models::runtime::ModelComputeRuntime::shared(),
    );
    let result = model
        .embed(
            &[vec![Content::Text("find relevant code".to_owned())]],
            EmbeddingOptions {
                on_progress: Some(Arc::new(move |progress| {
                    if let ModelProgress::Warning { message, .. } = progress {
                        lock_std_mutex(&captured).push(message);
                    }
                })),
                ..EmbeddingOptions::default()
            },
        )
        .await
        .expect("ORT CPU inference");
    assert_eq!(result.vectors[0].len(), entry.dimension);
    let loaded = model.state.lock().await;
    let provider = loaded
        .as_ref()
        .map(|loaded| loaded.sessions.provider())
        .expect("loaded model");
    assert_eq!(provider, TransformersExecutionProvider::Cpu);
    let warnings = lock_std_mutex(&warnings);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("using ORT CPU instead"));
}

#[test]
fn token_budget_packer_respects_batch_size_and_token_budget() {
    let lengths = [200, 5, 100, 6, 1];
    let batches = pack_token_budgeted_batches(&lengths, 2, 300);
    // Longest-first packing: the 200-token input cannot share a batch (two
    // padded rows would cost 400 tokens), the rest pair up by size.
    assert_eq!(batches, vec![vec![0], vec![2, 3], vec![1, 4]]);
    let mut covered = batches.concat();
    covered.sort_unstable();
    assert_eq!(covered, vec![0, 1, 2, 3, 4]);
    for batch in &batches {
        assert!(batch.len() <= 2);
        if batch.len() > 1 {
            let longest = batch
                .iter()
                .map(|&index| lengths[index])
                .max()
                .expect("non-empty batch");
            assert!(batch.len() * longest <= 300);
        }
    }
}

#[test]
fn token_budget_packer_handles_empty_single_and_oversized_inputs() {
    assert!(pack_token_budgeted_batches(&[], 4, MAX_BATCH_TOKENS).is_empty());
    assert_eq!(
        pack_token_budgeted_batches(&[7], 4, MAX_BATCH_TOKENS),
        vec![vec![0]]
    );
    // A single sequence longer than the budget still runs alone rather than
    // being dropped or split mid-sequence.
    assert_eq!(
        pack_token_budgeted_batches(&[9000, 3], 4, 4096),
        vec![vec![0], vec![1]]
    );
    // Zero-length inputs still occupy one padded position per batch.
    assert_eq!(
        pack_token_budgeted_batches(&[0, 0, 0, 0, 0], 2, 4096),
        vec![vec![0, 1], vec![2, 3], vec![4]]
    );
}

#[test]
fn overlapping_leases_queue_on_the_aggregate_thread_budget() {
    let mut budget = CpuThreadBudget::new(8);
    // A limit-1 lease claims its all-cores session.
    assert!(budget.fits(Some(8)));
    budget.acquire(Some(8));
    // A concurrent limit-4 lease's cores/4 session cannot start while the
    // machine is fully subscribed: it queues instead of oversubscribing,
    // which is the exact interleaving the per-session checks alone missed.
    assert!(!budget.fits(Some(2)));
    // Non-CPU sessions carry no thread budget.
    assert!(budget.fits(None));
    budget.acquire(None);
    // When the limit-1 lease completes, the smaller session fits again.
    budget.release(Some(8));
    assert!(budget.fits(Some(2)));
    // Four limit-4 sessions saturate the budget; a fifth queues.
    for _ in 0..4 {
        assert!(budget.fits(Some(2)));
        budget.acquire(Some(2));
    }
    assert!(!budget.fits(Some(2)));
    // Release is matched to acquire in practice; it clamps rather than
    // underflowing.
    budget.release(Some(100));
    assert_eq!(budget.active, 0);
    assert!(budget.fits(Some(8)));
}

#[test]
fn cpu_intra_threads_divides_available_cores_and_never_reaches_zero() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    assert_eq!(cpu_intra_threads(0), cores);
    assert_eq!(cpu_intra_threads(1), cores);
    assert_eq!(cpu_intra_threads(cores), 1);
    assert_eq!(cpu_intra_threads(cores.saturating_mul(4)), 1);
    assert!(cpu_intra_threads(3) >= 1);
}

#[test]
fn pooled_cpu_sessions_never_exceed_the_callers_execution_budget() {
    // A session created for a limit-1 lease (all cores) must not serve a
    // limit-4 lease (cores/4): reusing it would oversubscribe cores.
    assert!(!cpu_session_within_budget(Some(10), 2));
    // Sessions at or below the caller's implied budget stay reusable, so a
    // query after a concurrent index run reuses the smaller index sessions
    // instead of forcing a rebuild (underutilization, never oversubscription).
    assert!(cpu_session_within_budget(Some(2), 10));
    assert!(cpu_session_within_budget(Some(4), 4));
    assert!(cpu_session_within_budget(Some(1), 1));
    // Non-CPU sessions carry no intra-op budget.
    assert!(cpu_session_within_budget(None, 1));
    // The claim-side check also accepts non-CPU slots and CPU pools without a
    // desired budget, and re-checks the budget before the busy claim so a
    // session that goes idle mid-sweep cannot be handed out oversized.
    assert!(cpu_session_claimable(None, Some(1)));
    assert!(cpu_session_claimable(Some(4), None));
    assert!(cpu_session_claimable(Some(2), Some(4)));
    assert!(!cpu_session_claimable(Some(10), Some(2)));
}

fn sub_batch_result(call: usize, texts: &[String]) -> EmbeddingResult {
    #[allow(clippy::cast_precision_loss)]
    let vectors = texts
        .iter()
        .enumerate()
        .map(|(local, _)| vec![call as f32, local as f32, 0.0])
        .collect::<Vec<_>>();
    EmbeddingResult {
        vectors,
        truncated: if call == 1 { vec![1] } else { Vec::new() },
    }
}

#[test]
fn cpu_sub_batches_reassemble_vectors_in_input_order() {
    let entry = entry("mean", true);
    let texts = (0..10)
        .map(|index| format!("fragment {index}"))
        .collect::<Vec<_>>();
    // Mixed lengths so sorting reorders inputs across sub-batches
    // (max_batch_size 4 -> three sub-batches of longest-first indexes).
    let lengths = [10, 1, 9, 2, 8, 3, 7, 4, 6, 5];
    let calls = StdMutex::new(Vec::new());
    let result = embed_sub_batches(&texts, &lengths, entry, None, |indexes, sub_texts| {
        let call = lock_std_mutex(&calls).len();
        lock_std_mutex(&calls).push((indexes.to_vec(), sub_texts.to_vec()));
        Ok(sub_batch_result(call, sub_texts))
    })
    .expect("sub-batched embedding");
    assert_eq!(
        *lock_std_mutex(&calls),
        vec![
            (
                vec![0, 2, 4, 6],
                vec![
                    texts[0].clone(),
                    texts[2].clone(),
                    texts[4].clone(),
                    texts[6].clone()
                ]
            ),
            (
                vec![8, 9, 7, 5],
                vec![
                    texts[8].clone(),
                    texts[9].clone(),
                    texts[7].clone(),
                    texts[5].clone()
                ]
            ),
            (vec![3, 1], vec![texts[3].clone(), texts[1].clone()]),
        ]
    );
    // Each vector lands at its original input index with its sub-batch
    // coordinates [call, local], proving the reassembly order.
    assert_eq!(result.vectors[0], vec![0.0, 0.0, 0.0]);
    assert_eq!(result.vectors[1], vec![2.0, 1.0, 0.0]);
    assert_eq!(result.vectors[2], vec![0.0, 1.0, 0.0]);
    assert_eq!(result.vectors[3], vec![2.0, 0.0, 0.0]);
    assert_eq!(result.vectors[4], vec![0.0, 2.0, 0.0]);
    assert_eq!(result.vectors[5], vec![1.0, 3.0, 0.0]);
    assert_eq!(result.vectors[6], vec![0.0, 3.0, 0.0]);
    assert_eq!(result.vectors[7], vec![1.0, 2.0, 0.0]);
    assert_eq!(result.vectors[8], vec![1.0, 0.0, 0.0]);
    assert_eq!(result.vectors[9], vec![1.0, 1.0, 0.0]);
    assert_eq!(result.vectors.len(), texts.len());
    // Truncation is reported for original index 9 (local index 1 of batch 1).
    assert_eq!(result.truncated, vec![9]);
}

#[test]
fn cpu_sub_batches_check_cancellation_between_sub_batches() {
    let entry = entry("mean", true);
    let texts = (0..9)
        .map(|index| format!("fragment {index}"))
        .collect::<Vec<_>>();
    let lengths = [1; 9];
    let signal = CancellationToken::new();
    let calls = StdMutex::new(0_usize);
    let error = embed_sub_batches(
        &texts,
        &lengths,
        entry,
        Some(&signal),
        |_indexes, sub_texts| {
            let mut calls = lock_std_mutex(&calls);
            *calls += 1;
            if *calls == 1 {
                signal.cancel();
            }
            Ok(EmbeddingResult {
                vectors: vec![vec![0.0; 3]; sub_texts.len()],
                truncated: Vec::new(),
            })
        },
    )
    .expect_err("cancelled between sub-batches");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    // The first sub-batch ran; the remaining two were skipped.
    assert_eq!(*lock_std_mutex(&calls), 1);
}

#[test]
fn cpu_sub_batches_accept_empty_input() {
    let result = embed_sub_batches(
        &[],
        &[],
        entry("mean", true),
        None,
        |_indexes, _sub_texts| panic!("no sub-batches should run for empty input"),
    )
    .expect("empty input");
    assert!(result.vectors.is_empty());
    assert!(result.truncated.is_empty());
}
