//! `call_adapter` — the single entry point every consumer (lope, chat,
//! swarm) goes through. Walks transport → output → verdict. Failures at
//! either layer are converted into `ValidatorResult { status: INFRA_ERROR }`
//! so callers never have to distinguish "adapter crashed" from "adapter
//! voted FAIL".

use std::time::Duration;

use thiserror::Error;

use super::manifest::Manifest;
use super::output::{parse_response, OutputError};
use super::result::ValidatorResult;
use super::transport::{call_transport, CallContext, TransportError};

#[derive(Debug, Error)]
pub enum AdapterCallError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Output(#[from] OutputError),
}

/// Call an adapter with the given prompt. Never panics; any infrastructure
/// problem becomes a [`ValidatorResult`] with `INFRA_ERROR` status.
///
/// `timeout_seconds = None` uses the default 60s.
pub async fn call_adapter(
    manifest: &Manifest,
    prompt: &str,
    ctx: CallContext,
) -> ValidatorResult {
    let adapter_name = manifest.adapter.name.as_str();
    let transport_result = call_transport(manifest, prompt, &ctx).await;
    let response = match transport_result {
        Ok(r) => r,
        Err(e) => {
            return ValidatorResult::infra_error(
                adapter_name,
                format!("transport: {e}"),
            );
        }
    };

    match parse_response(manifest, &response.body, response.meta.duration) {
        Ok(mut r) => {
            // Sanity: propagate the observed duration onto the inner
            // PhaseVerdict so lope pools can sort/threshold on it.
            if r.verdict.duration_seconds == 0.0 {
                r.verdict.duration_seconds = response.meta.duration.as_secs_f64();
            }
            r
        }
        Err(e) => ValidatorResult::infra_error(adapter_name, format!("output parser: {e}")),
    }
}

/// Variant that applies a default timeout when the context doesn't set one.
pub async fn call_adapter_with_default_timeout(
    manifest: &Manifest,
    prompt: &str,
    default_timeout: Duration,
) -> ValidatorResult {
    let ctx = CallContext {
        timeout_seconds: Some(default_timeout.as_secs()),
        env: None,
    };
    call_adapter(manifest, prompt, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Manifest;

    fn minimal_manifest(base_url: &str, format: &str, verdict_field_line: &str) -> Manifest {
        let body = format!(
            r#"
[adapter]
name = "testadapter"
version = "0.1.0"
manifest_schema = 1
description = "t"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "{base_url}"

[auth]
scheme = "none"

[output]
format = "{format}"
{verdict_field_line}

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "network-io"
"#,
        );
        Manifest::parse_str(&body).unwrap()
    }

    /// Sanity smoke: unroutable URL produces INFRA_ERROR verdict, not a
    /// panic, not a Rust `Err`.
    #[tokio::test(flavor = "multi_thread")]
    async fn dead_endpoint_becomes_infra_error() {
        let m = minimal_manifest(
            "http://127.0.0.1:1/v1",
            "openai-chat",
            "verdict_field = \"choices.0.message.content\"",
        );
        let ctx = CallContext::default().with_timeout(1);
        let r = call_adapter(&m, "test prompt", ctx).await;
        assert_eq!(
            r.verdict.status,
            crate::adapter::result::VerdictStatus::InfraError
        );
        assert!(!r.ok());
    }
}
