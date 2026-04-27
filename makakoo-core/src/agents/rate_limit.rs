//! Phase 12 — token-bucket rate limiter.
//!
//! Locked Q13:
//!
//! * Per `(slot_id, transport_id, sender_key)` bucket — 60 messages
//!   per 5 minutes default. `sender_key` is transport-specific
//!   (Telegram user_id, Slack U…, Discord user_id, Web visitor_id,
//!   WhatsApp wa_id, Email From-address, Voice E.164).
//! * Second tier: per-slot global limit (600 / 5min) defends against
//!   multi-account spam where sender identity is weak (Email,
//!   WhatsApp).
//! * Webhook verification probes (Slack URL verification, WhatsApp
//!   `hub.challenge`, Twilio status callbacks) bypass the limiter
//!   entirely — not subject to inbound rate at all.
//! * On limit hit: emit a `system_message` frame with
//!   `text="rate_limited"` and `display_message="<custom_or_default>"`.
//!   Python gateway relays `display_message` verbatim — no LLM call.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Locked default per-sender capacity.
pub const DEFAULT_PER_SENDER: u32 = 60;
/// Locked default per-slot global capacity (sum across senders).
pub const DEFAULT_PER_SLOT: u32 = 600;
/// Locked window for both tiers.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Per-bucket state.
#[derive(Debug)]
struct Bucket {
    /// Tokens remaining in the current window.
    tokens: u32,
    /// When the current window started.
    window_start: Instant,
    /// Capacity — used to refill on window roll.
    capacity: u32,
}

impl Bucket {
    fn new(capacity: u32) -> Self {
        Self {
            tokens: capacity,
            window_start: Instant::now(),
            capacity,
        }
    }

    /// Try to consume one token. Refills if the window has rolled.
    fn try_consume(&mut self, now: Instant, window: Duration) -> bool {
        if now.duration_since(self.window_start) >= window {
            self.tokens = self.capacity;
            self.window_start = now;
        }
        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }
}

/// Composite key for the per-sender bucket.
pub fn sender_key(slot_id: &str, transport_id: &str, sender: &str) -> String {
    format!("{slot_id}::{transport_id}::{sender}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// Frame admitted. Continue normal handling.
    Admit,
    /// Per-sender bucket exhausted. The reply protocol applies
    /// (system_message frame).
    DenySender,
    /// Per-slot global bucket exhausted (rare; signals a multi-
    /// account attack on a weak-identity transport).
    DenyGlobal,
}

/// In-process limiter state. One instance per running supervisor.
#[derive(Debug)]
pub struct RateLimiter {
    per_sender: Mutex<HashMap<String, Bucket>>,
    per_slot: Mutex<HashMap<String, Bucket>>,
    sender_capacity: u32,
    slot_capacity: u32,
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::with_locked_defaults()
    }
}

impl RateLimiter {
    pub fn with_locked_defaults() -> Self {
        Self::new(DEFAULT_PER_SENDER, DEFAULT_PER_SLOT, DEFAULT_WINDOW)
    }

    pub fn new(sender_capacity: u32, slot_capacity: u32, window: Duration) -> Self {
        Self {
            per_sender: Mutex::new(HashMap::new()),
            per_slot: Mutex::new(HashMap::new()),
            sender_capacity,
            slot_capacity,
            window,
        }
    }

    /// Check + consume one token. Order: per-slot first (cheaper to
    /// trip on global flood), then per-sender.
    pub fn check_and_consume(
        &self,
        slot_id: &str,
        transport_id: &str,
        sender: &str,
    ) -> RateDecision {
        self.check_and_consume_at(slot_id, transport_id, sender, Instant::now())
    }

    /// Test-injectable variant.
    pub fn check_and_consume_at(
        &self,
        slot_id: &str,
        transport_id: &str,
        sender: &str,
        now: Instant,
    ) -> RateDecision {
        // Per-slot global tier.
        {
            let mut slot_buckets = self.per_slot.lock().unwrap();
            let bucket = slot_buckets
                .entry(slot_id.to_string())
                .or_insert_with(|| Bucket::new(self.slot_capacity));
            if !bucket.try_consume(now, self.window) {
                return RateDecision::DenyGlobal;
            }
        }
        // Per-sender tier.
        let key = sender_key(slot_id, transport_id, sender);
        let mut sender_buckets = self.per_sender.lock().unwrap();
        let bucket = sender_buckets
            .entry(key)
            .or_insert_with(|| Bucket::new(self.sender_capacity));
        if bucket.try_consume(now, self.window) {
            RateDecision::Admit
        } else {
            // Refund the per-slot token we consumed since the per-
            // sender bucket denied. Avoids skewed accounting on a
            // single sender flooding past their limit.
            let mut slot_buckets = self.per_slot.lock().unwrap();
            if let Some(b) = slot_buckets.get_mut(slot_id) {
                b.tokens = b.tokens.saturating_add(1).min(b.capacity);
            }
            RateDecision::DenySender
        }
    }
}

/// Locked reply text the gateway relays verbatim on rate-limit hit
/// when no slot-custom message is set.
pub const DEFAULT_RATE_LIMIT_MESSAGE: &str =
    "I'm receiving messages too fast — please slow down.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_message_is_admitted() {
        let l = RateLimiter::with_locked_defaults();
        assert_eq!(
            l.check_and_consume("secretary", "telegram-main", "1"),
            RateDecision::Admit
        );
    }

    #[test]
    fn per_sender_limit_denies_after_capacity() {
        let l = RateLimiter::new(3, 100, Duration::from_secs(60));
        for _ in 0..3 {
            assert_eq!(
                l.check_and_consume("secretary", "t", "user1"),
                RateDecision::Admit
            );
        }
        assert_eq!(
            l.check_and_consume("secretary", "t", "user1"),
            RateDecision::DenySender
        );
    }

    #[test]
    fn per_slot_global_limit_denies_after_capacity() {
        let l = RateLimiter::new(100, 3, Duration::from_secs(60));
        // Three different senders consume the global budget.
        for sender in ["u1", "u2", "u3"] {
            assert_eq!(
                l.check_and_consume("secretary", "t", sender),
                RateDecision::Admit
            );
        }
        // Fourth sender hits the global cap.
        assert_eq!(
            l.check_and_consume("secretary", "t", "u4"),
            RateDecision::DenyGlobal
        );
    }

    #[test]
    fn separate_slots_have_separate_buckets() {
        let l = RateLimiter::new(1, 1, Duration::from_secs(60));
        assert_eq!(
            l.check_and_consume("secretary", "t", "x"),
            RateDecision::Admit
        );
        assert_eq!(
            l.check_and_consume("career", "t", "x"),
            RateDecision::Admit
        );
    }

    #[test]
    fn separate_transports_have_separate_buckets() {
        let l = RateLimiter::new(1, 100, Duration::from_secs(60));
        assert_eq!(
            l.check_and_consume("secretary", "telegram-main", "u"),
            RateDecision::Admit
        );
        assert_eq!(
            l.check_and_consume("secretary", "slack-main", "u"),
            RateDecision::Admit
        );
        // Same sender on telegram-main is now exhausted (cap=1).
        assert_eq!(
            l.check_and_consume("secretary", "telegram-main", "u"),
            RateDecision::DenySender
        );
    }

    #[test]
    fn per_sender_deny_refunds_global_token() {
        // Cap per-sender at 1, per-slot at 3. After u1 consumes its
        // single token, a second u1 message must DenySender AND not
        // burn a global token (so two distinct senders can still
        // fit afterwards).
        let l = RateLimiter::new(1, 3, Duration::from_secs(60));
        assert_eq!(l.check_and_consume("s", "t", "u1"), RateDecision::Admit);
        assert_eq!(
            l.check_and_consume("s", "t", "u1"),
            RateDecision::DenySender
        );
        // After the refund, two more distinct senders both fit
        // (global has 2 tokens left: 3 - 1 consumed by u1).
        assert_eq!(l.check_and_consume("s", "t", "u2"), RateDecision::Admit);
        assert_eq!(l.check_and_consume("s", "t", "u3"), RateDecision::Admit);
    }

    #[test]
    fn window_roll_replenishes_tokens() {
        let l = RateLimiter::new(2, 100, Duration::from_secs(60));
        let t0 = Instant::now();
        l.check_and_consume_at("s", "t", "u", t0);
        l.check_and_consume_at("s", "t", "u", t0);
        assert_eq!(
            l.check_and_consume_at("s", "t", "u", t0),
            RateDecision::DenySender
        );
        // Cross the window — bucket refills.
        let t_later = t0 + Duration::from_secs(61);
        assert_eq!(
            l.check_and_consume_at("s", "t", "u", t_later),
            RateDecision::Admit
        );
    }

    #[test]
    fn sender_key_includes_all_three_dimensions() {
        let k = sender_key("secretary", "telegram-main", "U001");
        assert_eq!(k, "secretary::telegram-main::U001");
    }
}
