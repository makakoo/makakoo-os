//! Truncation (to at most 512 dimensions) and symmetric absmax `i8`
//! quantization for compact nearest-neighbour folder indexes.

use crate::error::{MakakooError, Result};

use super::{validate_dim, ChunkInput};

/// Truncate an embedder float vector to `target_dim` (<= 512) and quantize
/// symmetrically to `i8` in `[-127, 127]`.
///
/// Quantization preserves direction (and therefore cosine similarity) because
/// scaling by `127.0 / max_abs` is a positive scalar multiple of the truncated
/// vector. Rejects `target_dim == 0`, `target_dim > 512`, vectors shorter than
/// `target_dim`, and non-finite (`NaN`/`Inf`) components.
pub fn truncate_and_quantize_i8(input: &[f32], target_dim: usize) -> Result<Vec<i8>> {
    validate_dim(target_dim)?;
    if input.len() < target_dim {
        return Err(MakakooError::invalid_input(format!(
            "embedding length {} is shorter than target dim {target_dim}",
            input.len()
        )));
    }
    let prefix = &input[..target_dim];
    let mut max_abs: f32 = 0.0;
    for (idx, &v) in prefix.iter().enumerate() {
        if !v.is_finite() {
            return Err(MakakooError::invalid_input(format!(
                "embedding contains non-finite value at index {idx}"
            )));
        }
        let a = v.abs();
        if a > max_abs {
            max_abs = a;
        }
    }
    if max_abs == 0.0 {
        return Ok(vec![0i8; target_dim]);
    }
    let scale = 127.0_f32 / max_abs;
    let out = prefix
        .iter()
        .map(|&v| (v * scale).round().clamp(-127.0, 127.0) as i8)
        .collect();
    Ok(out)
}

pub(crate) fn quantize_chunks(chunks: &[ChunkInput], dim: usize) -> Result<Vec<Vec<i8>>> {
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.char_end < chunk.char_start {
            return Err(MakakooError::invalid_input(format!(
                "chunk char_end ({}) < char_start ({})",
                chunk.char_end, chunk.char_start
            )));
        }
        out.push(truncate_and_quantize_i8(&chunk.vector, dim)?);
    }
    Ok(out)
}

/// View an `i8` vector as raw bytes (1 byte per dimension, 512 bytes for 512d)
/// for compact SQLite BLOB storage.
pub fn i8_slice_to_bytes(vec: &[i8]) -> &[u8] {
    // SAFETY: `i8` and `u8` have identical size (1) and alignment (1), and
    // every bit pattern is valid for both types.
    unsafe { std::slice::from_raw_parts(vec.as_ptr() as *const u8, vec.len()) }
}

/// Decode raw SQLite BLOB bytes back into an `i8` vector.
pub fn bytes_to_i8_vec(bytes: &[u8]) -> Vec<i8> {
    bytes.iter().map(|&b| b as i8).collect()
}

/// Exact cosine similarity between two `i8` vectors in `[-1.0, 1.0]`.
pub fn cosine_similarity_i8(a: &[i8], b: &[i8]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot: f64 = 0.0;
    let mut na: f64 = 0.0;
    let mut nb: f64 = 0.0;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let xf = x as f64;
        let yf = y as f64;
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    let denom = (na * nb).sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0) as f32
    }
}

/// Convert a USearch `MetricKind::Cos` distance (`1.0 - cosine_similarity`)
/// into a higher-is-better cosine similarity score in `[-1.0, 1.0]`.
pub fn distance_to_cosine_score(distance: f32) -> f32 {
    if !distance.is_finite() {
        return 0.0;
    }
    (1.0 - distance).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superbrain::folder_index::MAX_VECTOR_DIM;

    #[test]
    fn rejects_zero_dim() {
        let err = truncate_and_quantize_i8(&[1.0, 2.0], 0).unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[test]
    fn rejects_dim_over_512() {
        let big = vec![0.5_f32; MAX_VECTOR_DIM + 1];
        let err = truncate_and_quantize_i8(&big, MAX_VECTOR_DIM + 1).unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[test]
    fn rejects_short_input_vector() {
        let short = vec![0.1_f32; 128];
        let err = truncate_and_quantize_i8(&short, 512).unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[test]
    fn rejects_nan_and_inf_components() {
        let mut with_nan = vec![0.1_f32; 8];
        with_nan[3] = f32::NAN;
        assert!(truncate_and_quantize_i8(&with_nan, 8).is_err());

        let mut with_inf = vec![0.1_f32; 8];
        with_inf[5] = f32::INFINITY;
        assert!(truncate_and_quantize_i8(&with_inf, 8).is_err());

        let mut with_neg_inf = vec![0.1_f32; 8];
        with_neg_inf[0] = f32::NEG_INFINITY;
        assert!(truncate_and_quantize_i8(&with_neg_inf, 8).is_err());
    }

    #[test]
    fn truncates_3072_to_512_and_preserves_cosine_ranking() {
        let make_vec = |seed: u64| -> Vec<f32> {
            let mut x = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (0..3072)
                .map(|_| {
                    x = x
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let bits = ((x >> 40) as i32) - (1 << 23);
                    (bits as f32) / ((1 << 23) as f32)
                })
                .collect()
        };
        let query = make_vec(1);
        let mut near = query.clone();
        for (i, x) in near.iter_mut().enumerate().take(512) {
            if i % 16 == 0 {
                *x *= 0.9;
            }
        }
        let far = make_vec(42);

        let q8 = truncate_and_quantize_i8(&query, 512).unwrap();
        let near8 = truncate_and_quantize_i8(&near, 512).unwrap();
        let far8 = truncate_and_quantize_i8(&far, 512).unwrap();

        assert_eq!(q8.len(), 512);
        let sim_self = cosine_similarity_i8(&q8, &q8);
        let sim_near = cosine_similarity_i8(&q8, &near8);
        let sim_far = cosine_similarity_i8(&q8, &far8);
        assert!(sim_self > 0.999);
        assert!(sim_near > 0.99);
        assert!(sim_near > sim_far + 0.3);
    }

    #[test]
    fn blob_roundtrip_is_exact() {
        let orig: Vec<i8> = (-127..=127).collect();
        let raw = i8_slice_to_bytes(&orig);
        assert_eq!(raw.len(), orig.len());
        let decoded = bytes_to_i8_vec(raw);
        assert_eq!(decoded, orig);
    }

    #[test]
    fn zero_vector_quantizes_to_all_zeros() {
        let zeros = vec![0.0_f32; 16];
        let q = truncate_and_quantize_i8(&zeros, 16).unwrap();
        assert_eq!(q, vec![0i8; 16]);
    }
}
