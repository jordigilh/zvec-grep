use std::{collections::HashMap, path::Path};

use mlx_rs::{Array, Dtype};

use super::{HIDDEN_SIZE, JinaWeights, tensor};
use crate::models::spi::ModelError;

fn code(error: &ModelError) -> &'static str {
    error.code()
}

#[test]
fn a_missing_file_is_a_structured_storage_failure() {
    let error = JinaWeights::load(
        Path::new("/nonexistent/model.safetensors"),
        &tokio_util::sync::CancellationToken::new(),
    )
    .err()
    .expect("missing weights");
    assert_eq!(code(&error), crate::EngineError::STORAGE_FAILURE);
}

#[test]
fn a_missing_tensor_is_a_structured_storage_failure() {
    let tensors = HashMap::new();
    let error = JinaWeights::from_tensors(&tensors, &tokio_util::sync::CancellationToken::new())
        .err()
        .expect("empty tensor map");
    assert_eq!(code(&error), crate::EngineError::STORAGE_FAILURE);
    assert!(
        error
            .cause()
            .expect("missing tensor detail")
            .contains("tensor=")
    );
}

#[test]
fn an_unexpected_tensor_shape_is_a_structured_storage_failure() {
    let mut tensors = HashMap::new();
    tensors.insert(
        "embedding".to_owned(),
        Array::from_slice(&[0.0_f32, 1.0, 2.0], &[3]),
    );
    let error = tensor(&tensors, "embedding", &[2]).expect_err("wrong shape");
    assert_eq!(code(&error), crate::EngineError::STORAGE_FAILURE);
}

#[test]
fn loaded_tensors_are_converted_to_fp16() {
    let mut tensors = HashMap::new();
    tensors.insert(
        "embedding".to_owned(),
        Array::from_slice(
            &vec![0.5_f32; (HIDDEN_SIZE * HIDDEN_SIZE) as usize],
            &[HIDDEN_SIZE, HIDDEN_SIZE],
        ),
    );
    let array = tensor(&tensors, "embedding", &[HIDDEN_SIZE, HIDDEN_SIZE]).expect("valid tensor");
    assert_eq!(array.dtype(), Dtype::Float16);
}
