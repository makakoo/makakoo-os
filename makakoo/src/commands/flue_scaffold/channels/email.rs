//! Email channel renderer — DEFERRED to V2.
//!
//! Per codex review of Phase 4, there is no first-party `@flue/email`
//! package on npm, and the spec's `email` channel kind has no
//! canonical Flue adapter. Generating a stub TS file would mislead
//! operators into thinking the channel works.
//!
//! V1 behavior: surface a clear error at scaffold time. The
//! operator should use a `webhook` channel + their own `defineTool`
//! for SMTP/IMAP, or wait for V2 when/if a first-party package
//! ships.

pub fn render(_smtp_host: &str, _imap_host: &str, _secret_env: &str) -> anyhow::Result<String> {
    Err(anyhow::anyhow!(
        "spec declares an `email` channel, but the @flue/* adapter is not available in V1. \
         Use a `webhook` channel + a custom `defineTool` for SMTP/IMAP, or remove the channel \
         from the spec. Tracked for V2."
    ))
}
