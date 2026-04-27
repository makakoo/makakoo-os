//! OpenAI-compatible LLM client for switchAILocal.
//!
//! Routes chat completions, tool-calling loops, multimodal describe_* calls,
//! and image generation through a single `reqwest::Client`. Applies
//! exponential backoff with jitter on 429/5xx, up to `max_retries`.
//!
//! The multimodal helpers (`describe_image`, `describe_audio`,
//! `describe_video`) always target `xiaomi-tp:mimo-v2-omni` via the chat
//! endpoint — this matches the behaviour of the Python `core.llm.omni`
//! module so callers get identical semantics across languages.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{MakakooError, Result};

pub const DEFAULT_BASE_URL: &str = "http://localhost:18080/v1";
pub const OMNI_MODEL: &str = "xiaomi-tp:mimo-v2-omni";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    max_retries: u32,
    #[allow(dead_code)]
    timeout: Duration,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    /// Construct with env-driven defaults (`AIL_BASE_URL`, `AIL_API_KEY`).
    pub fn new() -> Self {
        let base_url = std::env::var("AIL_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let api_key = std::env::var("AIL_API_KEY").ok().filter(|s| !s.is_empty());
        let timeout = Duration::from_secs(120);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        Self {
            base_url,
            api_key,
            client,
            max_retries: 3,
            timeout,
        }
    }

    /// Construct with an explicit base URL (tests, injected configs).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut c = Self::new();
        c.base_url = base_url.into();
        c
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn set_max_retries(&mut self, n: u32) {
        self.max_retries = n;
    }

    fn auth_header(&self) -> Option<String> {
        self.api_key.as_ref().map(|k| format!("Bearer {k}"))
    }

    /// Simple chat completion. Returns the assistant message content.
    pub async fn chat(&self, model: &str, messages: Vec<ChatMessage>) -> Result<String> {
        let body = json!({
            "model": model,
            "messages": messages,
        });
        let resp = self.post_with_retry("/chat/completions", &body).await?;
        extract_content(&resp)
            .ok_or_else(|| MakakooError::llm("no content in chat response"))
    }

    /// Chat completion with tools, returning both content and tool calls.
    pub async fn chat_with_tools(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<Tool>,
    ) -> Result<ChatResponse> {
        let body = json!({
            "model": model,
            "messages": messages,
            "tools": tools,
        });
        let resp = self.post_with_retry("/chat/completions", &body).await?;
        let content = extract_content(&resp);
        let tool_calls = extract_tool_calls(&resp);
        Ok(ChatResponse { content, tool_calls })
    }

    /// Image understanding via mimo-v2-omni.
    pub async fn describe_image(&self, source: &str, prompt: &str) -> Result<String> {
        let source_val = self.encode_source(source, "image").await?;
        let body = json!({
            "model": OMNI_MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": source_val } }
                ]
            }],
        });
        let resp = self.post_with_retry("/chat/completions", &body).await?;
        extract_content(&resp)
            .ok_or_else(|| MakakooError::llm("no content in describe_image response"))
    }

    /// Audio understanding via mimo-v2-omni.
    pub async fn describe_audio(&self, source: &str, prompt: &str) -> Result<String> {
        let source_val = self.encode_source(source, "audio").await?;
        let body = json!({
            "model": OMNI_MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "input_audio", "input_audio": { "data": source_val, "format": "wav" } }
                ]
            }],
        });
        let resp = self.post_with_retry("/chat/completions", &body).await?;
        extract_content(&resp)
            .ok_or_else(|| MakakooError::llm("no content in describe_audio response"))
    }

    /// Video understanding via mimo-v2-omni.
    pub async fn describe_video(
        &self,
        source: &str,
        prompt: &str,
        fps: Option<f32>,
    ) -> Result<String> {
        let source_val = self.encode_source(source, "video").await?;
        let mut video_url = json!({ "url": source_val });
        if let Some(fps) = fps {
            video_url["fps"] = json!(fps);
        }
        let body = json!({
            "model": OMNI_MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "video_url", "video_url": video_url }
                ]
            }],
        });
        let resp = self.post_with_retry("/chat/completions", &body).await?;
        extract_content(&resp)
            .ok_or_else(|| MakakooError::llm("no content in describe_video response"))
    }

    /// Text-to-image. Returns raw PNG bytes.
    pub async fn generate_image(&self, prompt: &str, size: &str) -> Result<Vec<u8>> {
        let body = json!({
            "model": "ail-image",
            "prompt": prompt,
            "size": size,
            "response_format": "b64_json",
        });
        let resp = self.post_with_retry("/images/generations", &body).await?;
        let b64 = resp
            .get("data")
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("b64_json"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MakakooError::llm("no b64_json in image response"))?;
        B64.decode(b64)
            .map_err(|e| MakakooError::llm(format!("base64 decode failed: {e}")))
    }

    /// Accept a URL, data URI, or local path. Paths become `data:` URIs
    /// with a best-effort mime type inferred from the caller's modality
    /// hint. URLs and existing data URIs pass through untouched.
    async fn encode_source(&self, source: &str, kind: &str) -> Result<String> {
        if source.starts_with("http://")
            || source.starts_with("https://")
            || source.starts_with("data:")
        {
            return Ok(source.to_string());
        }
        let path = Path::new(source);
        if !path.exists() {
            return Err(MakakooError::NotFound(format!(
                "media source not found: {source}"
            )));
        }
        let bytes = tokio::fs::read(path).await?;
        let encoded = B64.encode(&bytes);
        let mime = guess_mime(path, kind);
        Ok(format!("data:{mime};base64,{encoded}"))
    }

    /// POST JSON with exponential-backoff retry on 429/5xx. Returns the
    /// parsed JSON body on success.
    async fn post_with_retry(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt: u32 = 0;
        loop {
            let mut req = self.client.post(&url).json(body);
            if let Some(auth) = self.auth_header() {
                req = req.header("Authorization", auth);
            }
            let result = req.send().await;
            let should_retry = match &result {
                Ok(resp) => {
                    let s = resp.status();
                    s.as_u16() == 429 || s.is_server_error()
                }
                Err(e) => e.is_timeout() || e.is_connect(),
            };
            if should_retry && attempt < self.max_retries {
                let delay_ms = backoff_ms(attempt);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                attempt += 1;
                continue;
            }
            let resp = result?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(MakakooError::llm(format!(
                    "http {status}: {text}"
                )));
            }
            let v: Value = resp.json().await?;
            return Ok(v);
        }
    }
}

fn backoff_ms(attempt: u32) -> u64 {
    // Deterministic pseudo-jitter keyed off the attempt number so tests
    // remain reproducible without pulling in a full RNG crate.
    let base = 200u64 << attempt; // 200, 400, 800, ...
    let jitter = (attempt as u64 * 37) % 100;
    base + jitter
}

fn guess_mime(path: &Path, kind: &str) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match (kind, ext.as_str()) {
        ("image", "png") => "image/png",
        ("image", "jpg") | ("image", "jpeg") => "image/jpeg",
        ("image", "gif") => "image/gif",
        ("image", "webp") => "image/webp",
        ("image", _) => "image/png",
        ("audio", "wav") => "audio/wav",
        ("audio", "mp3") => "audio/mpeg",
        ("audio", "ogg") => "audio/ogg",
        ("audio", "flac") => "audio/flac",
        ("audio", _) => "audio/wav",
        ("video", "mp4") => "video/mp4",
        ("video", "webm") => "video/webm",
        ("video", "mov") => "video/quicktime",
        ("video", _) => "video/mp4",
        _ => "application/octet-stream",
    }
}

fn extract_content(resp: &Value) -> Option<String> {
    resp.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

fn extract_tool_calls(resp: &Value) -> Vec<ToolCall> {
    resp.get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| serde_json::from_value::<ToolCall>(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn backoff_is_monotonic() {
        assert!(backoff_ms(0) < backoff_ms(1));
        assert!(backoff_ms(1) < backoff_ms(2));
    }

    #[test]
    fn guess_mime_image_png() {
        assert_eq!(guess_mime(Path::new("foo.png"), "image"), "image/png");
        assert_eq!(guess_mime(Path::new("x.jpg"), "image"), "image/jpeg");
        assert_eq!(guess_mime(Path::new("x.mp4"), "video"), "video/mp4");
        assert_eq!(guess_mime(Path::new("x.wav"), "audio"), "audio/wav");
    }

    #[test]
    fn chat_message_constructors() {
        assert_eq!(ChatMessage::user("hi").role, "user");
        assert_eq!(ChatMessage::system("hi").role, "system");
        assert_eq!(ChatMessage::assistant("hi").role, "assistant");
    }

    #[tokio::test]
    async fn chat_success_returns_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "hello from mock" }
                }]
            })))
            .mount(&server)
            .await;

        let client = LlmClient::with_base_url(server.uri());
        let out = client
            .chat("ail-compound", vec![ChatMessage::user("hi")])
            .await
            .unwrap();
        assert_eq!(out, "hello from mock");
    }

    #[tokio::test]
    async fn chat_retries_on_500_then_succeeds() {
        let server = MockServer::start().await;
        // First call: 500. Second call: 200. wiremock responds with the
        // first matching, unused stub, so order is controlled by mount
        // sequence.
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "ok" }
                }]
            })))
            .mount(&server)
            .await;

        let mut client = LlmClient::with_base_url(server.uri());
        client.set_max_retries(3);
        let out = client
            .chat("ail-compound", vec![ChatMessage::user("hi")])
            .await
            .unwrap();
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn chat_gives_up_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let mut client = LlmClient::with_base_url(server.uri());
        client.set_max_retries(1);
        let err = client
            .chat("ail-compound", vec![ChatMessage::user("hi")])
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("500") || msg.to_lowercase().contains("llm"));
    }

    #[tokio::test]
    async fn generate_image_decodes_b64() {
        let server = MockServer::start().await;
        let payload = b"\x89PNG\r\n\x1a\nFAKE";
        let b64 = B64.encode(payload);
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;

        let client = LlmClient::with_base_url(server.uri());
        let bytes = client.generate_image("a banana", "512x512").await.unwrap();
        assert_eq!(bytes, payload);
    }
}
