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

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use reqwest::{redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{MakakooError, Result};

pub const DEFAULT_BASE_URL: &str = "http://localhost:18080/v1";
pub const OMNI_MODEL: &str = "xiaomi-tp:mimo-v2-omni";
const API_KEY_ENV_NAMES: [&str; 3] = ["AIL_API_KEY", "SWITCHAI_KEY", "LLM_API_KEY"];
const BASE_URL_ENV_NAMES: [&str; 2] = ["AIL_BASE_URL", "LLM_BASE_URL"];
const IMAGE_CDN_ROOT: &str = "aliyuncs.com";
const MAX_GENERATED_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn first_non_empty_from<F>(names: &[&str], mut lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    names
        .iter()
        .find_map(|name| lookup(name).filter(|value| !value.trim().is_empty()))
}

fn resolve_api_key_with<F>(lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    first_non_empty_from(&API_KEY_ENV_NAMES, lookup)
}

fn resolve_base_url_with<F>(lookup: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    first_non_empty_from(&BASE_URL_ENV_NAMES, lookup)
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    image_download_client: reqwest::Client,
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
    /// Construct with env-driven defaults. Canonical AIL names win, then
    /// legacy switchAILocal-compatible aliases. Missing credentials remain
    /// valid for gateways that intentionally run without authentication.
    pub fn new() -> Self {
        let base_url = resolve_base_url_with(|name| std::env::var(name).ok());
        let api_key = resolve_api_key_with(|name| std::env::var(name).ok());
        let timeout = Duration::from_secs(120);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        let image_download_client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .build()
            .expect("reqwest image client build");
        Self {
            base_url,
            api_key,
            client,
            image_download_client,
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
        extract_content(&resp).ok_or_else(|| MakakooError::llm("no content in chat response"))
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
        Ok(ChatResponse {
            content,
            tool_calls,
        })
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

    /// Text-to-image. Supports OpenAI-style base64 responses and the
    /// URL/JPEG response currently returned by MiniMax through switchAILocal.
    /// When a provider returns multiple URLs, the first non-empty image wins.
    pub async fn generate_image(
        &self,
        prompt: &str,
        size: &str,
        aspect_ratio: Option<&str>,
    ) -> Result<GeneratedImage> {
        let mut body = json!({
            "model": "ail-image",
            "prompt": prompt,
            "size": size,
            "response_format": "b64_json",
        });
        if let Some(aspect_ratio) = aspect_ratio {
            body["aspect_ratio"] = json!(aspect_ratio);
        }
        let resp = self.post_with_retry("/images/generations", &body).await?;
        match extract_generated_image_payload(&resp)? {
            GeneratedImagePayload::Base64(b64) => {
                let bytes = B64
                    .decode(b64)
                    .map_err(|e| MakakooError::llm(format!("base64 decode failed: {e}")))?;
                if bytes.len() > MAX_GENERATED_IMAGE_BYTES {
                    return Err(MakakooError::llm(
                        "generated image exceeds 20 MiB response limit",
                    ));
                }
                generated_image_from_bytes(bytes)
            }
            GeneratedImagePayload::Url(url) => {
                self.download_generated_image(url, MAX_GENERATED_IMAGE_BYTES)
                    .await
            }
        }
    }

    async fn download_generated_image(
        &self,
        raw_url: &str,
        max_bytes: usize,
    ) -> Result<GeneratedImage> {
        let validated = self.validate_generated_image_url(raw_url)?;
        let mut request = self.image_download_client.get(validated.url);
        if validated.same_gateway_origin {
            if let Some(auth) = self.auth_header() {
                request = request.header("Authorization", auth);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|error| sanitized_download_error("request", &validated.host, &error))?;
        let status = response.status();
        if !status.is_success() {
            return Err(MakakooError::llm(format!(
                "image download from host {} returned {status}",
                validated.host
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(MakakooError::llm(format!(
                "image download from host {} exceeds {} byte limit",
                validated.host, max_bytes
            )));
        }
        let has_image_content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("image/"));
        if !has_image_content_type {
            return Err(MakakooError::llm(format!(
                "image download from host {} returned a non-image content type",
                validated.host
            )));
        }

        let initial_capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(max_bytes);
        let mut bytes = Vec::with_capacity(initial_capacity);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| sanitized_download_error("body", &validated.host, &error))?;
            append_bounded_image_chunk(&mut bytes, &chunk, max_bytes, &validated.host)?;
        }
        generated_image_from_bytes(bytes)
    }

    fn validate_generated_image_url(&self, raw_url: &str) -> Result<ValidatedImageUrl> {
        let url = Url::parse(raw_url)
            .map_err(|_| MakakooError::llm("image response contained an invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(MakakooError::llm(
                "image response URL must use http or https",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(MakakooError::llm(
                "image response URL must not contain user information",
            ));
        }
        if url.fragment().is_some() {
            return Err(MakakooError::llm(
                "image response URL must not contain a fragment",
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| MakakooError::llm("image response URL has no host"))?
            .to_string();
        let gateway = Url::parse(&self.base_url)
            .map_err(|_| MakakooError::llm("configured gateway URL is invalid"))?;
        let same_gateway_origin = same_origin(&url, &gateway);
        let allowed_cdn = is_allowed_image_cdn_host(&host)
            && url.port_or_known_default() == default_port(url.scheme());
        if !same_gateway_origin && !allowed_cdn {
            return Err(MakakooError::llm(format!(
                "image response URL host {host} is not allowed"
            )));
        }
        Ok(ValidatedImageUrl {
            url,
            host,
            same_gateway_origin,
        })
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
                return Err(MakakooError::llm(format!("http {status}: {text}")));
            }
            let v: Value = resp.json().await?;
            return Ok(v);
        }
    }
}

enum GeneratedImagePayload<'a> {
    Base64(&'a str),
    Url(&'a str),
}

#[derive(Debug)]
struct ValidatedImageUrl {
    url: Url,
    host: String,
    same_gateway_origin: bool,
}

fn extract_generated_image_payload(resp: &Value) -> Result<GeneratedImagePayload<'_>> {
    let data = resp.get("data");
    let first = data.and_then(|value| value.get(0));
    if let Some(b64) = first
        .and_then(|value| value.get("b64_json"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(GeneratedImagePayload::Base64(b64));
    }
    if let Some(url) = first
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(GeneratedImagePayload::Url(url));
    }
    if let Some(url) = data
        .and_then(|value| value.get("image_urls"))
        .and_then(Value::as_array)
        .and_then(|urls| {
            urls.iter()
                .filter_map(Value::as_str)
                .find(|value| !value.trim().is_empty())
        })
    {
        return Ok(GeneratedImagePayload::Url(url));
    }
    Err(MakakooError::llm(
        "image response contained no supported image payload",
    ))
}

fn generated_image_from_bytes(bytes: Vec<u8>) -> Result<GeneratedImage> {
    let mime_type = detect_image_mime(&bytes)
        .ok_or_else(|| MakakooError::llm("generated image has an unsupported byte format"))?;
    Ok(GeneratedImage {
        bytes,
        mime_type: mime_type.to_string(),
    })
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn append_bounded_image_chunk(
    bytes: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    host: &str,
) -> Result<()> {
    if bytes.len().saturating_add(chunk.len()) > max_bytes {
        return Err(MakakooError::llm(format!(
            "image download from host {host} exceeds {max_bytes} byte limit"
        )));
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_allowed_image_cdn_host(host: &str) -> bool {
    host == IMAGE_CDN_ROOT || host.ends_with(&format!(".{IMAGE_CDN_ROOT}"))
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn sanitized_download_error(stage: &str, host: &str, error: &reqwest::Error) -> MakakooError {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failure"
    } else if error.is_body() {
        "body read failure"
    } else if error.is_decode() {
        "decode failure"
    } else {
        "request failure"
    };
    MakakooError::llm(format!(
        "image download {stage} failed for host {host}: {kind}"
    ))
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
    use std::collections::BTreeMap;
    use wiremock::matchers::{body_partial_json, header, method, path};
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

    #[test]
    fn api_key_resolution_uses_documented_precedence_and_skips_empty_values() {
        let vars = BTreeMap::from([
            ("AIL_API_KEY", "   "),
            ("SWITCHAI_KEY", "switch-key"),
            ("LLM_API_KEY", "legacy-key"),
            ("OPENAI_API_KEY", "must-not-be-used"),
        ]);
        let resolved = resolve_api_key_with(|name| vars.get(name).map(ToString::to_string));
        assert_eq!(resolved.as_deref(), Some("switch-key"));

        let vars = BTreeMap::from([
            ("AIL_API_KEY", "ail-key"),
            ("SWITCHAI_KEY", "switch-key"),
            ("LLM_API_KEY", "legacy-key"),
        ]);
        let resolved = resolve_api_key_with(|name| vars.get(name).map(ToString::to_string));
        assert_eq!(resolved.as_deref(), Some("ail-key"));
    }

    #[test]
    fn api_key_resolution_ignores_openai_key_and_allows_no_key() {
        let vars = BTreeMap::from([("OPENAI_API_KEY", "not-a-gateway-key")]);
        assert_eq!(
            resolve_api_key_with(|name| vars.get(name).map(ToString::to_string)),
            None
        );
    }

    #[test]
    fn base_url_resolution_falls_back_from_empty_ail_to_legacy_then_default() {
        let vars = BTreeMap::from([
            ("AIL_BASE_URL", ""),
            ("LLM_BASE_URL", "http://legacy.test/v1"),
        ]);
        assert_eq!(
            resolve_base_url_with(|name| vars.get(name).map(ToString::to_string)),
            "http://legacy.test/v1"
        );
        assert_eq!(resolve_base_url_with(|_| None), DEFAULT_BASE_URL);
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
        let image = client
            .generate_image("a banana", "512x512", None)
            .await
            .unwrap();
        assert_eq!(image.bytes, payload);
        assert_eq!(image.mime_type, "image/png");
    }

    #[tokio::test]
    async fn generate_image_downloads_data_zero_url_and_forwards_auth_same_origin() {
        let server = MockServer::start().await;
        let payload = b"\xff\xd8\xff\xe0SYNTHETIC-JPEG";
        let image_url = format!("{}/generated.jpg?signature=synthetic", server.uri());
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .and(body_partial_json(json!({ "aspect_ratio": "16:9" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "url": image_url }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/generated.jpg"))
            .and(header("authorization", "Bearer synthetic-switch-test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(payload),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut client = LlmClient::with_base_url(server.uri());
        let vars = BTreeMap::from([
            ("AIL_API_KEY", ""),
            ("SWITCHAI_KEY", "synthetic-switch-test-key"),
            ("LLM_API_KEY", "synthetic-legacy-test-key"),
        ]);
        client.api_key = resolve_api_key_with(|name| vars.get(name).map(ToString::to_string));
        let image = client
            .generate_image("cover art", "1024x1024", Some("16:9"))
            .await
            .unwrap();
        assert_eq!(image.bytes, payload);
        assert_eq!(image.mime_type, "image/jpeg");
    }

    #[tokio::test]
    async fn generate_image_uses_first_non_empty_image_urls_entry() {
        let server = MockServer::start().await;
        let payload = b"GIF89aSYNTHETIC";
        let image_url = format!("{}/generated.gif", server.uri());
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "image_urls": ["", image_url, "https://unused.invalid/image.gif"] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/generated.gif"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/gif")
                    .set_body_bytes(payload),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut client = LlmClient::with_base_url(server.uri());
        client.api_key = None;
        let image = client
            .generate_image("animation", "1024x1024", None)
            .await
            .unwrap();
        assert_eq!(image.bytes, payload);
        assert_eq!(image.mime_type, "image/gif");
    }

    #[test]
    fn image_payload_precedence_prefers_b64_over_urls() {
        let payload = B64.encode(b"\x89PNG\r\n\x1a\nSYNTHETIC");
        let response = json!({
            "data": [{
                "b64_json": payload,
                "url": "https://unused.invalid/image.png"
            }]
        });
        assert!(matches!(
            extract_generated_image_payload(&response).unwrap(),
            GeneratedImagePayload::Base64(_)
        ));
    }

    #[test]
    fn image_mime_detection_supports_only_documented_formats() {
        assert_eq!(detect_image_mime(b"\x89PNG\r\n\x1a\nX"), Some("image/png"));
        assert_eq!(detect_image_mime(b"\xff\xd8\xffX"), Some("image/jpeg"));
        assert_eq!(detect_image_mime(b"GIF87aX"), Some("image/gif"));
        assert_eq!(
            detect_image_mime(b"RIFF\x00\x00\x00\x00WEBPX"),
            Some("image/webp")
        );
        assert_eq!(detect_image_mime(b"<svg></svg>"), None);
    }

    #[test]
    fn url_policy_allows_same_origin_and_minimax_cdn_without_leaking_auth() {
        let client = LlmClient::with_base_url("http://localhost:18080/v1");
        let same = client
            .validate_generated_image_url("http://localhost:18080/generated.png")
            .unwrap();
        assert!(same.same_gateway_origin);

        let cdn = client
            .validate_generated_image_url(
                "http://bucket.example.aliyuncs.com/generated.jpg?signature=synthetic",
            )
            .unwrap();
        assert!(!cdn.same_gateway_origin);
        assert_eq!(cdn.host, "bucket.example.aliyuncs.com");

        for rejected in [
            "https://aliyuncs.com.evil.example/generated.jpg?signature=synthetic",
            "http://bucket.aliyuncs.com:8080/generated.jpg?signature=synthetic",
            "file:///tmp/generated.jpg",
            "https://user:pass@bucket.aliyuncs.com/generated.jpg",
            "https://bucket.aliyuncs.com/generated.jpg#fragment",
        ] {
            let err = client.validate_generated_image_url(rejected).unwrap_err();
            let message = err.to_string();
            assert!(!message.contains("signature=synthetic"));
            assert!(!message.contains("user:pass"));
        }
    }

    #[tokio::test]
    async fn image_download_rejects_redirects_and_non_image_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/final.png", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/text"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_bytes(b"not an image"),
            )
            .mount(&server)
            .await;

        let client = LlmClient::with_base_url(server.uri());
        let redirect = client
            .download_generated_image(&format!("{}/redirect", server.uri()), 1024)
            .await
            .unwrap_err();
        assert!(redirect.to_string().contains("302"));
        let non_image = client
            .download_generated_image(&format!("{}/text", server.uri()), 1024)
            .await
            .unwrap_err();
        assert!(non_image.to_string().contains("non-image"));
    }

    #[tokio::test]
    async fn image_download_enforces_declared_size_and_magic_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/large.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(b"\x89PNG\r\n\x1a\nTOO-LARGE"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/unknown"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(b"not-a-supported-image-format"),
            )
            .mount(&server)
            .await;

        let client = LlmClient::with_base_url(server.uri());
        let oversized = client
            .download_generated_image(&format!("{}/large.png", server.uri()), 8)
            .await
            .unwrap_err();
        assert!(oversized.to_string().contains("8 byte limit"));
        let unknown = client
            .download_generated_image(&format!("{}/unknown", server.uri()), 1024)
            .await
            .unwrap_err();
        assert!(unknown.to_string().contains("unsupported byte format"));
    }

    #[test]
    fn streamed_image_chunks_cannot_cross_hard_cap() {
        let mut bytes = vec![1, 2, 3, 4];
        append_bounded_image_chunk(&mut bytes, &[5, 6], 6, "mock.local").unwrap();
        let err = append_bounded_image_chunk(&mut bytes, &[7], 6, "mock.local").unwrap_err();
        assert!(err.to_string().contains("6 byte limit"));
        assert_eq!(bytes, vec![1, 2, 3, 4, 5, 6]);
    }

    #[tokio::test]
    async fn empty_image_response_returns_bounded_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;
        let client = LlmClient::with_base_url(server.uri());
        let err = client
            .generate_image("nothing", "1024x1024", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no supported image payload"));
    }
}
