// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Symmetric int8 quantisation utilities.
//!
//! The default search payload ships **f32** embeddings (4 bytes per
//! dim) — small enough at `dim=256` for a typical site corpus that the
//! extra plumbing for runtime int8 dequant isn't justified. The
//! helpers in this module exist for two reasons:
//!
//! 1. When the `model2vec` feature is enabled, the upstream model
//!    weights ARE int8-quantised and we need to round-trip them
//!    correctly into `model.bin`.
//! 2. Future work: int8-encoded embeddings (issue follow-up) — this
//!    module gives us the building blocks without a second crate.
//!
//! Quantisation is **per-vector symmetric**: pick `scale = max(|x|)`,
//! map every component to `round(x / scale * 127)` clamped to `[-127,
//! 127]`. The `scale` is stored alongside the int8 payload so the
//! decoder can dequantise to f32 if needed. L2 norm is preserved up to
//! the int8 rounding noise (≈ 0.4% RMSE at dim=256).

/// Result of quantising a single vector: scale + i8 payload.
///
/// # Examples
///
/// ```
/// use ssg_search::quantize::{QuantizedVector, dequantize_int8};
///
/// // Manually construct a quantised vector representing [1.0, -1.0, 0.0].
/// let q = QuantizedVector {
///     scale: 1.0 / 127.0,
///     data: vec![127, -127, 0],
/// };
/// let back = dequantize_int8(&q);
/// assert!((back[0] - 1.0).abs() < 1e-6);
/// assert!((back[1] + 1.0).abs() < 1e-6);
/// assert_eq!(back[2], 0.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedVector {
    /// Scale factor — multiply each `i8` component by this to recover
    /// the original f32 value (up to int8 rounding).
    pub scale: f32,
    /// Quantised payload, same length as the source vector.
    pub data: Vec<i8>,
}

/// Symmetric per-vector int8 quantisation.
///
/// Returns a zero-scale, zero-payload result for an empty input, and a
/// zero-scale result for an all-zero vector — the decoder treats both
/// as the zero vector.
///
/// # Examples
///
/// ```
/// use ssg_search::quantize::quantize_int8;
///
/// // Symmetric: max-abs element maps to +/-127.
/// let q = quantize_int8(&[1.0, 0.5, -0.25]);
/// assert_eq!(q.data[0], 127);
/// assert!((q.scale - 1.0 / 127.0).abs() < 1e-7);
///
/// // All-zero input → zero scale, zero payload.
/// let zero = quantize_int8(&[0.0, 0.0, 0.0]);
/// assert_eq!(zero.scale, 0.0);
/// assert_eq!(zero.data, vec![0i8; 3]);
///
/// // Empty input → empty result.
/// let empty = quantize_int8(&[]);
/// assert!(empty.data.is_empty());
/// ```
#[must_use]
pub fn quantize_int8(vec: &[f32]) -> QuantizedVector {
    if vec.is_empty() {
        return QuantizedVector {
            scale: 0.0,
            data: Vec::new(),
        };
    }
    let mut max_abs: f32 = 0.0;
    for &x in vec {
        let a = x.abs();
        if a > max_abs {
            max_abs = a;
        }
    }
    if max_abs == 0.0 {
        return QuantizedVector {
            scale: 0.0,
            data: vec![0i8; vec.len()],
        };
    }
    let scale = max_abs / 127.0;
    let inv = 1.0_f32 / scale;
    let data: Vec<i8> = vec
        .iter()
        .map(|x| {
            let q = (x * inv).round();
            let c = q.clamp(-127.0, 127.0);
            c as i8
        })
        .collect();
    QuantizedVector { scale, data }
}

/// Inverse of [`quantize_int8`]. Reconstructs the f32 vector up to
/// the symmetric-int8 rounding error.
///
/// # Examples
///
/// ```
/// use ssg_search::quantize::{quantize_int8, dequantize_int8};
///
/// // Round-trip preserves values within int8 rounding error.
/// let original = vec![0.7_f32, -0.3, 0.1, 1.0, -1.0];
/// let q = quantize_int8(&original);
/// let back = dequantize_int8(&q);
/// assert_eq!(back.len(), original.len());
/// for (a, b) in original.iter().zip(&back) {
///     assert!((a - b).abs() <= q.scale / 2.0 + 1e-6);
/// }
/// ```
#[must_use]
pub fn dequantize_int8(q: &QuantizedVector) -> Vec<f32> {
    q.data.iter().map(|&b| f32::from(b) * q.scale).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trip() {
        let q = quantize_int8(&[]);
        assert_eq!(q.scale, 0.0);
        assert_eq!(q.data, Vec::<i8>::new());
        assert_eq!(dequantize_int8(&q), Vec::<f32>::new());
    }

    #[test]
    fn all_zero_round_trip() {
        let q = quantize_int8(&[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(q.scale, 0.0);
        assert_eq!(q.data, vec![0i8; 4]);
        assert_eq!(dequantize_int8(&q), vec![0.0_f32; 4]);
    }

    #[test]
    fn round_trip_preserves_within_int8_rounding() {
        let input: Vec<f32> =
            (0..256).map(|i| ((i as f32) / 256.0).sin()).collect();
        let q = quantize_int8(&input);
        let back = dequantize_int8(&q);
        let max_err: f32 = input
            .iter()
            .zip(&back)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        // Per-component rounding error bound: scale / 2.
        assert!(max_err <= q.scale / 2.0 + 1e-6, "max_err {max_err}");
    }

    #[test]
    fn quantize_clamps_to_i8_range() {
        // After we craft a scale, every output must fit in i8.
        let input = vec![1.0_f32, -1.0, 0.5, -0.5];
        let q = quantize_int8(&input);
        for v in &q.data {
            // i8 tops out at 127, so only the -128 end can be missed.
            assert_ne!(*v, i8::MIN);
        }
    }

    #[test]
    fn quantize_max_maps_to_127() {
        let input = vec![1.0_f32, 0.5, -0.25];
        let q = quantize_int8(&input);
        assert_eq!(q.data[0], 127);
    }

    #[test]
    fn quantize_min_maps_to_neg_127() {
        let input = vec![-1.0_f32, 0.5, 0.25];
        let q = quantize_int8(&input);
        assert_eq!(q.data[0], -127);
    }

    #[test]
    fn round_trip_preserves_dimension() {
        for n in [1usize, 8, 64, 256, 1024] {
            let input: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
            let q = quantize_int8(&input);
            assert_eq!(q.data.len(), n);
            assert_eq!(dequantize_int8(&q).len(), n);
        }
    }

    #[test]
    fn quantize_with_single_negative_max_yields_neg_127() {
        // Single element with abs max negative — should produce -127.
        let q = quantize_int8(&[-3.5_f32]);
        assert_eq!(q.data, vec![-127]);
        assert!(q.scale > 0.0);
    }

    #[test]
    fn quantize_scale_is_max_abs_over_127() {
        let input = vec![0.5_f32, -2.0, 1.0];
        let q = quantize_int8(&input);
        let expected_scale = 2.0_f32 / 127.0;
        assert!((q.scale - expected_scale).abs() < 1e-7);
    }

    #[test]
    fn quantize_vector_clone_and_eq() {
        let q = quantize_int8(&[1.0, -1.0, 0.5]);
        let cloned = q.clone();
        assert_eq!(q, cloned);
    }

    #[test]
    fn quantize_vector_eq_distinguishes_scale() {
        let q1 = QuantizedVector {
            scale: 1.0,
            data: vec![1, 2, 3],
        };
        let q2 = QuantizedVector {
            scale: 2.0,
            data: vec![1, 2, 3],
        };
        assert_ne!(q1, q2);
    }

    #[test]
    fn dequantize_zero_scale_produces_zeros() {
        let q = QuantizedVector {
            scale: 0.0,
            data: vec![10, -20, 30],
        };
        let back = dequantize_int8(&q);
        assert_eq!(back, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn dequantize_recovers_known_payload() {
        let q = QuantizedVector {
            scale: 0.1,
            data: vec![10, -10, 0, 50],
        };
        let back = dequantize_int8(&q);
        assert!((back[0] - 1.0).abs() < 1e-6);
        assert!((back[1] + 1.0).abs() < 1e-6);
        assert_eq!(back[2], 0.0);
        assert!((back[3] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn dequantize_empty_vector() {
        let q = QuantizedVector {
            scale: 0.5,
            data: vec![],
        };
        assert_eq!(dequantize_int8(&q), Vec::<f32>::new());
    }

    #[test]
    fn quantize_single_zero_returns_zero_scale() {
        let q = quantize_int8(&[0.0_f32]);
        assert_eq!(q.scale, 0.0);
        assert_eq!(q.data, vec![0]);
    }

    #[test]
    fn quantize_extreme_values_clamp_to_range() {
        // Even with tiny non-max values, every output must stay in
        // [-127, 127].
        let input = vec![f32::MIN_POSITIVE, -1e30, 1e30];
        let q = quantize_int8(&input);
        for &v in &q.data {
            // i8 tops out at 127, so only the -128 end can be missed.
            assert_ne!(v, i8::MIN);
        }
    }

    #[test]
    fn dequantize_then_requantize_is_stable_at_boundary() {
        // After one round-trip the data lands on grid points, so a
        // second round-trip should reproduce the same i8 payload.
        let input = vec![0.7_f32, -0.3, 0.1, 1.0, -1.0];
        let q1 = quantize_int8(&input);
        let back = dequantize_int8(&q1);
        let q2 = quantize_int8(&back);
        assert_eq!(q1.data, q2.data);
    }
}
