//! Tier-C olibia handler — `harvey_olibia_speak`.
//!
//! Renders a mascot gimmick frame for the supplied context string and,
//! when TTS is available on the host OS, speaks the text through the
//! platform synthesizer. Both paths are best-effort: the handler returns
//! `{spoken, frame}` regardless of whether the TTS binary is installed,
//! so MCP clients running in headless environments (Codex, Qwen, Linux
//! containers without `espeak`) still see a structured response.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use makakoo_core::chat::tts;
use makakoo_core::gimmicks;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

pub struct HarveyOlibiaSpeakHandler {
    #[allow(dead_code)]
    ctx: Arc<ToolContext>,
}

impl HarveyOlibiaSpeakHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveyOlibiaSpeakHandler {
    fn name(&self) -> &str {
        "harvey_olibia_speak"
    }
    fn description(&self) -> &str {
        "Olibia the guardian owl speaks. Renders a mascot gimmick frame \
         for the given context and (when TTS is available) speaks the \
         text through the host's system synthesizer. Returns \
         { spoken, frame }."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "What Olibia should say" },
                "mood": { "type": "string", "description": "Optional mood tag used for gimmick selection" },
                "silent": { "type": "boolean", "description": "If true, skip TTS and only return the frame" }
            },
            "required": ["text"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RpcError::invalid_params("missing or empty 'text'"))?
            .to_string();
        let mood = params
            .get("mood")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("neutral");
        let silent = params
            .get("silent")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Gimmick frame is best-effort — cooldown might swallow it, and
        // force=false honours the 5-minute pacing.
        let frame = gimmicks::render_gimmick(mood, false)
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("[{mood}] {text}"));

        let mut spoken = false;
        if !silent {
            match tts::speak(&text) {
                Ok(()) => spoken = true,
                Err(e) => {
                    tracing::debug!(
                        target: "makakoo.olibia.speak",
                        "tts::speak failed ({e}); returning silent response"
                    );
                }
            }
        }

        Ok(json!({
            "spoken": spoken,
            "frame": frame,
            "mood": mood,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(PathBuf::from("/tmp/mkk-olibia-test")))
    }

    #[tokio::test]
    async fn silent_mode_returns_frame_without_speaking() {
        let h = HarveyOlibiaSpeakHandler::new(ctx());
        let out = h
            .call(json!({"text": "hoot hoot", "silent": true, "mood": "happy"}))
            .await
            .unwrap();
        assert!(!out["spoken"].as_bool().unwrap());
        assert!(!out["frame"].as_str().unwrap().is_empty());
        assert_eq!(out["mood"].as_str().unwrap(), "happy");
    }

    #[tokio::test]
    async fn missing_text_is_invalid_params() {
        let h = HarveyOlibiaSpeakHandler::new(ctx());
        let err = h.call(json!({"silent": true})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }
}
