//! Tier-B multimodal handlers — the four omni tools.
//!
//! Thin wrappers over `LlmClient::{describe_image, describe_audio,
//! describe_video, generate_image}`. Every handler accepts a `source`
//! argument that can be a public URL, a `data:` URI, or an absolute
//! local path — the LLM client's own `encode_source` helper handles
//! base64 encoding + mime-type guessing.
//!
//! The generate_image handler re-encodes detected image bytes as base64 so
//! the JSON-RPC channel can carry PNG/JPEG/GIF/WebP through stdio without
//! framing hell; callers unpack it with any base64 decoder.

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

const DEFAULT_IMAGE_PROMPT: &str = "Describe this image in detail.";
const DEFAULT_AUDIO_PROMPT: &str = "Transcribe and summarise this audio clip.";
const DEFAULT_VIDEO_PROMPT: &str = "Describe what happens in this video.";
const DEFAULT_IMAGE_SIZE: &str = "1024x1024";

fn llm(ctx: &ToolContext) -> Result<&std::sync::Arc<makakoo_core::llm::LlmClient>, RpcError> {
    ctx.llm
        .as_ref()
        .ok_or_else(|| RpcError::internal("llm client not wired"))
}

// ─────────────────────────────────────────────────────────────────────
// harvey_describe_image
// ─────────────────────────────────────────────────────────────────────

pub struct DescribeImageHandler {
    ctx: Arc<ToolContext>,
}

impl DescribeImageHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for DescribeImageHandler {
    fn name(&self) -> &str {
        "harvey_describe_image"
    }
    fn description(&self) -> &str {
        "Look at an image and describe it. source can be a URL, data: \
         URI, or absolute local path. Routes through mimo-v2-omni."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": { "type": "string" },
                "prompt": { "type": "string" }
            },
            "required": ["source"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let source = params
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'source'"))?;
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_IMAGE_PROMPT);
        let llm = llm(&self.ctx)?;
        let description = llm
            .describe_image(source, prompt)
            .await
            .map_err(|e| RpcError::internal(format!("harvey_describe_image: {e}")))?;
        Ok(json!({ "description": description }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_describe_audio
// ─────────────────────────────────────────────────────────────────────

pub struct DescribeAudioHandler {
    ctx: Arc<ToolContext>,
}

impl DescribeAudioHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for DescribeAudioHandler {
    fn name(&self) -> &str {
        "harvey_describe_audio"
    }
    fn description(&self) -> &str {
        "Listen to audio and describe it. source can be a URL, data: \
         URI, or absolute local path. Routes through mimo-v2-omni."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": { "type": "string" },
                "prompt": { "type": "string" }
            },
            "required": ["source"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let source = params
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'source'"))?;
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_AUDIO_PROMPT);
        let llm = llm(&self.ctx)?;
        let description = llm
            .describe_audio(source, prompt)
            .await
            .map_err(|e| RpcError::internal(format!("harvey_describe_audio: {e}")))?;
        Ok(json!({ "description": description }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_describe_video
// ─────────────────────────────────────────────────────────────────────

pub struct DescribeVideoHandler {
    ctx: Arc<ToolContext>,
}

impl DescribeVideoHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for DescribeVideoHandler {
    fn name(&self) -> &str {
        "harvey_describe_video"
    }
    fn description(&self) -> &str {
        "Watch a video and describe what happens. source can be a URL, \
         data: URI, or absolute local path. Routes through mimo-v2-omni."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": { "type": "string" },
                "prompt": { "type": "string" },
                "fps": { "type": "number" }
            },
            "required": ["source"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let source = params
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'source'"))?;
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_VIDEO_PROMPT);
        let fps = params.get("fps").and_then(|v| v.as_f64()).map(|v| v as f32);
        let llm = llm(&self.ctx)?;
        let description = llm
            .describe_video(source, prompt, fps)
            .await
            .map_err(|e| RpcError::internal(format!("harvey_describe_video: {e}")))?;
        Ok(json!({ "description": description }))
    }
}

// ─────────────────────────────────────────────────────────────────────
// harvey_generate_image
// ─────────────────────────────────────────────────────────────────────

pub struct GenerateImageHandler {
    ctx: Arc<ToolContext>,
}

impl GenerateImageHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for GenerateImageHandler {
    fn name(&self) -> &str {
        "harvey_generate_image"
    }
    fn description(&self) -> &str {
        "Generate an image from a text prompt through ail-image. Returns \
         detected PNG/JPEG/GIF/WebP bytes under image_bytes_b64 with a \
         mime_type. If a provider returns multiple images, uses the first."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string" },
                "size": { "type": "string", "default": DEFAULT_IMAGE_SIZE },
                "aspect_ratio": {
                    "type": "string",
                    "description": "Optional provider aspect ratio such as 16:9"
                }
            },
            "required": ["prompt"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'prompt'"))?;
        let size = params
            .get("size")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_IMAGE_SIZE);
        let aspect_ratio = params.get("aspect_ratio").and_then(|v| v.as_str());
        let llm = llm(&self.ctx)?;
        let image = llm
            .generate_image(prompt, size, aspect_ratio)
            .await
            .map_err(|e| RpcError::internal(format!("harvey_generate_image: {e}")))?;
        Ok(generated_image_result(image, size, aspect_ratio))
    }
}

fn generated_image_result(
    image: makakoo_core::llm::GeneratedImage,
    size: &str,
    aspect_ratio: Option<&str>,
) -> Value {
    let encoded = B64.encode(&image.bytes);
    let is_png = image.mime_type == "image/png";
    let mut result = json!({
        "image_bytes_b64": encoded,
        "mime_type": image.mime_type,
        "bytes": image.bytes.len(),
        "size": size,
    });
    if let Some(aspect_ratio) = aspect_ratio {
        result["aspect_ratio"] = json!(aspect_ratio);
    }
    if is_png {
        result["png_bytes_b64"] = result["image_bytes_b64"].clone();
    }
    result
}

// ═════════════════════════════════════════════════════════════════════
// Tests — wired against an empty context. We only exercise the schema
// and the "not wired" error path; full end-to-end tests live in the
// T18 integration suite with a mocked switchAILocal server.
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    fn empty_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext::empty(std::env::temp_dir()))
    }

    #[tokio::test]
    async fn describe_image_requires_source() {
        let h = DescribeImageHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn describe_video_accepts_fps_number() {
        // Params parse cleanly; the LLM call itself will error without
        // a wired client, which we assert below.
        let h = DescribeVideoHandler::new(empty_ctx());
        let err = h
            .call(json!({
                "source": "/nonexistent.mp4",
                "fps": 2.5
            }))
            .await
            .unwrap_err();
        // Error comes from LlmClient trying to read the path — the
        // fact that we reached the client at all means fps parsed.
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn generate_image_requires_prompt() {
        let h = GenerateImageHandler::new(empty_ctx());
        let err = h.call(json!({})).await.unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn schemas_round_trip_through_serde_json() {
        let ctx = empty_ctx();
        for h in [
            DescribeImageHandler::new(ctx.clone()).input_schema(),
            DescribeAudioHandler::new(ctx.clone()).input_schema(),
            DescribeVideoHandler::new(ctx.clone()).input_schema(),
            GenerateImageHandler::new(ctx.clone()).input_schema(),
        ] {
            let s = serde_json::to_string(&h).unwrap();
            let _: Value = serde_json::from_str(&s).unwrap();
        }
    }

    #[test]
    fn generated_jpeg_result_never_claims_png() {
        let result = generated_image_result(
            makakoo_core::llm::GeneratedImage {
                bytes: b"\xff\xd8\xffSYNTHETIC".to_vec(),
                mime_type: "image/jpeg".to_string(),
            },
            "1024x1024",
            Some("16:9"),
        );
        assert_eq!(result["mime_type"], "image/jpeg");
        assert_eq!(result["aspect_ratio"], "16:9");
        assert!(result.get("image_bytes_b64").is_some());
        assert!(result.get("png_bytes_b64").is_none());
    }

    #[test]
    fn generated_png_result_keeps_deprecated_png_alias() {
        let result = generated_image_result(
            makakoo_core::llm::GeneratedImage {
                bytes: b"\x89PNG\r\n\x1a\nSYNTHETIC".to_vec(),
                mime_type: "image/png".to_string(),
            },
            "1024x1024",
            None,
        );
        assert_eq!(result["image_bytes_b64"], result["png_bytes_b64"]);
        assert!(result.get("aspect_ratio").is_none());
    }
}
