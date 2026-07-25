//! Voice channel renderer — DEFERRED to V2.
//!
//! Per codex review of Phase 4, there is no first-party `@flue/voice`
//! package on npm, and Twilio's webhook signature verification +
//! Recording-URL basic-auth don't have a canonical Flue adapter.
//!
//! V1 behavior: surface a clear error at scaffold time. The
//! operator should use a `webhook` channel + their own `defineTool`
//! for Twilio, or wait for V2 when/if a first-party package
//! ships.

pub fn render(_twilio_account_sid_env: &str, _secret_env: &str) -> anyhow::Result<String> {
    Err(anyhow::anyhow!(
        "spec declares a `voice` channel, but the @flue/* adapter is not available in V1. \
         Use a `webhook` channel + a custom `defineTool` for Twilio, or remove the channel \
         from the spec. Tracked for V2."
    ))
}
