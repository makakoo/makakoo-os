//! Memory scorer — pure functions for recency, frequency, and combined scoring.
//!
//! Ports the decay math from `core/memory/memory_scorer.py` and the
//! composite blend used by MemoryStack + MemoryPromoter. All functions
//! are pure, stateless, and unit-tested against known fixtures.

use chrono::{DateTime, Utc};

/// Default half-life for recency decay (matches Python MemoryScorer).
pub const DEFAULT_HALF_LIFE_DAYS: f32 = 30.0;

/// Default saturation constant for frequency scoring (log1p).
pub const DEFAULT_FREQ_SATURATION: f32 = 20.0;

/// Combined-score weights (matches Python MemoryScorer topic/recency/frequency):
///   topic (similarity) 0.40
///   recency            0.30
///   frequency          0.15
///   (the remaining 0.15 = tag_overlap which is folded into similarity upstream)
///
/// `combined_score` below renormalizes to 1.0 across the three inputs it
/// actually accepts so callers that don't use tag_overlap still get a
/// balanced result.
pub const W_TOPIC: f32 = 0.40;
pub const W_RECENCY: f32 = 0.30;
pub const W_FREQUENCY: f32 = 0.15;

/// Exponential decay of recency: `0.5 ^ (age_days / half_life_days)`.
///
/// Returns 1.0 for `last_hit >= now`, and asymptotically approaches 0 as
/// age grows. Ported from `memory_scorer._recency_decay` + Python promoter
/// `_recency` (both use `exp(-(ln2/T) * age)` which is mathematically
/// identical to `0.5^(age/T)`).
pub fn recency_score(
    last_hit: DateTime<Utc>,
    now: DateTime<Utc>,
    half_life_days: f32,
) -> f32 {
    let age_seconds = (now - last_hit).num_seconds() as f32;
    if age_seconds <= 0.0 {
        return 1.0;
    }
    if half_life_days <= 0.0 {
        return 0.0;
    }
    let age_days = age_seconds / 86_400.0;
    0.5_f32.powf(age_days / half_life_days)
}

/// Log-scaled frequency score: `log1p(hit_count) / log1p(saturation)`, clamped [0,1].
///
/// Uses `math.log1p` scaling identical to Python memory_scorer and memory_promoter.
/// Despite the function name in the T5 spec ("1 - exp(-hit/saturation)"), the
/// Python ground truth uses `log1p(n)/log1p(saturation)` which gives a better
/// plateau shape for curated Brain content. We ship the Python behavior so
/// downstream scores match the oracle byte-for-byte.
pub fn frequency_score(hit_count: u32, saturation: f32) -> f32 {
    if hit_count == 0 {
        return 0.0;
    }
    if saturation <= 0.0 {
        return 0.0;
    }
    let n = (hit_count as f32).ln_1p();
    let d = saturation.ln_1p();
    if d == 0.0 {
        return 0.0;
    }
    (n / d).clamp(0.0, 1.0)
}

/// Weighted blend of topic-similarity, recency, and frequency.
///
/// Weights are renormalized to the three inputs (tag-overlap is
/// upstream), so the returned score is in `[0, 1]`.
pub fn combined_score(similarity: f32, recency: f32, frequency: f32) -> f32 {
    let total = W_TOPIC + W_RECENCY + W_FREQUENCY;
    let w_t = W_TOPIC / total;
    let w_r = W_RECENCY / total;
    let w_f = W_FREQUENCY / total;
    (similarity.clamp(0.0, 1.0) * w_t
        + recency.clamp(0.0, 1.0) * w_r
        + frequency.clamp(0.0, 1.0) * w_f)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn recency_is_one_at_zero_age() {
        let now = Utc::now();
        assert!((recency_score(now, now, 30.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recency_is_half_at_half_life() {
        let now = Utc::now();
        let past = now - Duration::days(30);
        let s = recency_score(past, now, 30.0);
        assert!((s - 0.5).abs() < 1e-4, "expected 0.5 got {s}");
    }

    #[test]
    fn recency_is_quarter_at_two_half_lives() {
        let now = Utc::now();
        let past = now - Duration::days(60);
        let s = recency_score(past, now, 30.0);
        assert!((s - 0.25).abs() < 1e-4, "expected 0.25 got {s}");
    }

    #[test]
    fn recency_clamped_to_one_for_future() {
        let now = Utc::now();
        let future = now + Duration::days(5);
        assert!((recency_score(future, now, 30.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn frequency_zero_for_no_hits() {
        assert_eq!(frequency_score(0, 20.0), 0.0);
    }

    #[test]
    fn frequency_grows_monotonically() {
        let a = frequency_score(1, 20.0);
        let b = frequency_score(5, 20.0);
        let c = frequency_score(20, 20.0);
        assert!(a < b);
        assert!(b < c);
        assert!(c <= 1.0);
        // 20 hits hits the saturation ceiling.
        assert!((c - 1.0).abs() < 1e-4, "expected ~1.0 got {c}");
    }

    #[test]
    fn combined_blends_inputs() {
        let s = combined_score(1.0, 1.0, 1.0);
        assert!((s - 1.0).abs() < 1e-6);
        let s0 = combined_score(0.0, 0.0, 0.0);
        assert!(s0.abs() < 1e-6);
        // Topic dominates: sim=1, others=0 → 0.40/0.85 ≈ 0.470
        let s_topic = combined_score(1.0, 0.0, 0.0);
        assert!((s_topic - (0.40 / 0.85)).abs() < 1e-4);
    }
}
