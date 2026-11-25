use mlx_rs::{Array, transforms::eval};

use super::super::weights::HEAD_COUNT;
use super::{alibi, layer_norm, mask_bias};

fn values(array: &Array) -> Vec<f32> {
    eval([array]).expect("evaluate");
    array.as_slice::<f32>().to_vec()
}

#[test]
fn alibi_uses_the_twelve_recovered_slopes() {
    let bias = alibi(3).expect("alibi");
    assert_eq!(bias.shape(), &[1, HEAD_COUNT, 3, 3]);
    let data = values(&bias);
    let head = (3 * 3) as usize;
    // Head 0 slope is -0.5, so distance 0 -> 0, 1 -> -0.5, 2 -> -1.0.
    assert!(data[0].abs() < 1e-6);
    assert!((data[1] - -0.5).abs() < 1e-6);
    assert!((data[2] - -1.0).abs() < 1e-6);
    // Ninth head (index 8) uses -2^-0.5.
    let ninth = data[8 * head..9 * head].to_vec();
    assert!((ninth[1] - -0.707_106_77).abs() < 1e-5);
}

#[test]
fn mask_bias_masks_padded_positions_additively() {
    let mask = Array::from_slice(&[1.0_f32, 0.0], &[1, 2]);
    let bias = mask_bias(&mask, 1, 2).expect("mask bias");
    assert_eq!(bias.shape(), &[1, 1, 1, 2]);
    let data = values(&bias);
    assert!(data[0].abs() < 1e-6);
    assert!(data[1] < -1.0e38);
}

#[test]
fn layer_norm_computes_statistics_in_fp32_and_returns_fp16() {
    let input = Array::from_slice(&[1.0_f32, 3.0], &[1, 2])
        .as_type::<half::f16>()
        .expect("fp16");
    let weight = Array::from_slice(&[1.0_f32], &[1]);
    let bias = Array::from_slice(&[0.0_f32], &[1]);
    let normalized = layer_norm(&input, &weight, &bias)
        .expect("layer norm")
        .as_type::<f32>()
        .expect("promote fp16 output");
    eval([&normalized]).expect("evaluate");
    let data = normalized.as_slice::<f32>();
    assert!((data[0] - -1.0).abs() < 1e-3);
    assert!((data[1] - 1.0).abs() < 1e-3);
}
