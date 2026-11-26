use mlx_rs::{Array, transforms::eval};

use super::super::weights::HEAD_COUNT;
use super::{ShapeCache, alibi, gated_activation, layer_norm, mask_bias, pool_and_normalize};

fn values(array: &Array) -> Vec<f32> {
    eval([array]).expect("evaluate");
    array.as_slice::<f32>().to_vec()
}

#[test]
fn gated_activation_multiplies_the_gate_by_gelu_of_the_up_projection() {
    // gate = 2, up = 1 -> 2 * gelu(1) = 2 * 0.8413447 = 1.6826895.
    // The common swapped form gelu(gate) * up would give 1.9544997 instead, so
    // this asserts the ordering, not just "some activation happened".
    let gate = Array::from_slice(&[2.0_f32], &[1]);
    let up = Array::from_slice(&[1.0_f32], &[1]);
    let value = values(&gated_activation(&gate, &up).expect("gated activation"));
    assert!(
        (value[0] - 1.682_689_5).abs() < 1e-5,
        "expected gate * gelu(up) = 1.6826895, got {}",
        value[0]
    );
}

#[test]
fn pool_and_normalize_averages_masked_positions_and_scales_to_unit_length() {
    // Two positions, the second masked out: pooled = [3, 4], |pooled| = 5.
    let hidden = Array::from_slice(&[3.0_f32, 4.0, 99.0, 99.0], &[1, 2, 2]);
    let mask_pool = Array::from_slice(&[1.0_f32, 0.0], &[1, 2, 1]);
    let mask_sum = Array::from_slice(&[1.0_f32], &[1, 1, 1]);
    let pooled = values(&pool_and_normalize(&hidden, &mask_pool, &mask_sum).expect("pooled"));
    assert_eq!(pooled.len(), 2);
    assert!((pooled[0] - 0.6).abs() < 1e-6, "got {:?}", pooled);
    assert!((pooled[1] - 0.8).abs() < 1e-6, "got {:?}", pooled);
}

#[test]
fn compiled_shape_cache_reuses_one_graph_per_shape() {
    let mut cache = ShapeCache::new(4);
    let mut builds = 0;
    for _ in 0..3 {
        let _value = *cache.slot((8, 256), || {
            builds += 1;
            builds
        });
    }
    assert_eq!(builds, 1, "the same shape must not rebuild the graph");

    let _other = *cache.slot((1, 128), || {
        builds += 1;
        builds
    });
    assert_eq!(builds, 2, "a new shape builds once");
    let _again = *cache.slot((8, 256), || {
        builds += 1;
        builds
    });
    assert_eq!(builds, 2, "a cached shape is still a hit after a new one");
}

#[test]
fn compiled_shape_cache_evicts_the_oldest_entry_at_capacity() {
    let mut cache = ShapeCache::new(2);
    for key in [(1, 128), (2, 128), (3, 128)] {
        let _value = *cache.slot(key, || 0);
    }
    let mut rebuilt = false;
    let _evicted = *cache.slot((1, 128), || {
        rebuilt = true;
        0
    });
    assert!(rebuilt, "the oldest shape is evicted at capacity");
    let mut kept_rebuilt = false;
    let _kept = *cache.slot((3, 128), || {
        kept_rebuilt = true;
        0
    });
    assert!(!kept_rebuilt, "the newest shape survives eviction");
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
    let normalized = layer_norm(&input, &weight, &bias).expect("layer norm");
    assert_eq!(
        normalized.dtype(),
        mlx_rs::Dtype::Float16,
        "layer-norm output must stay fp16 outside its internal fp32 statistics"
    );
    let normalized = normalized.as_type::<f32>().expect("promote fp16 output");
    eval([&normalized]).expect("evaluate");
    let data = normalized.as_slice::<f32>();
    assert!((data[0] - -1.0).abs() < 1e-3);
    assert!((data[1] - 1.0).abs() < 1e-3);
}
