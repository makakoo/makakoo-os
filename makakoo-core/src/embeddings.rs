//! Embeddings client for OpenAI-compatible `/embeddings` endpoints.
//!
//! Default model: `qwen3-embedding:0.6b` — matches the canonical Brain
//! sync model used by the reference install.
//!
//! All requests obey [`EmbedRequestLimits`]: at most [`DEFAULT_EMBED_MAX_BATCH`]
//! (16, env `MAKAKOO_EMBED_MAX_BATCH`) inputs per HTTP request and at most
//! [`DEFAULT_EMBED_MAX_INPUT_BYTES`] (2,000, env `MAKAKOO_EMBED_MAX_INPUT_BYTES`)
//! UTF-8 bytes per input, truncated strictly on a UTF-8 character boundary.

use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::OnceCell;
use serde_json::{json, Value};

use crate::error::{MakakooError, Result};
use crate::llm::DEFAULT_BASE_URL;

#[path = "embeddings_response.rs"]
mod response;

#[cfg(test)]
#[path = "embeddings_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "embeddings_index_tests.rs"]
mod index_tests;

pub const DEFAULT_EMBED_MODEL: &str = "qwen3-embedding:0.6b";
/// Existing default per-request timeout for embedding HTTP calls.
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(60);
/// Existing default per-request timeout for non-blocking model probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Default maximum number of inputs carried in a single `/embeddings` request.
pub const DEFAULT_EMBED_MAX_BATCH: usize = 16;
/// Environment variable overriding [`DEFAULT_EMBED_MAX_BATCH`] (positive integer).
pub const EMBED_MAX_BATCH_ENV: &str = "MAKAKOO_EMBED_MAX_BATCH";
/// Default maximum UTF-8 byte length of a single text input sent to `/embeddings`.
pub const DEFAULT_EMBED_MAX_INPUT_BYTES: usize = 2_000;
/// Environment variable overriding [`DEFAULT_EMBED_MAX_INPUT_BYTES`] (positive integer).
pub const EMBED_MAX_INPUT_BYTES_ENV: &str = "MAKAKOO_EMBED_MAX_INPUT_BYTES";

/// Single source of truth for `/embeddings` request batch-size and UTF-8 byte
/// truncation limits across Brain rebuilds, top-ups, folder indexes, and queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedRequestLimits {
    pub max_batch: usize,
    pub max_input_bytes: usize,
}

impl Default for EmbedRequestLimits {
    fn default() -> Self {
        Self {
            max_batch: DEFAULT_EMBED_MAX_BATCH,
            max_input_bytes: DEFAULT_EMBED_MAX_INPUT_BYTES,
        }
    }
}

impl EmbedRequestLimits {
    /// Resolve batch size and UTF-8 byte cap from `MAKAKOO_EMBED_MAX_BATCH` and
    /// `MAKAKOO_EMBED_MAX_INPUT_BYTES`, falling back to [`DEFAULT_EMBED_MAX_BATCH`]
    /// (16) and [`DEFAULT_EMBED_MAX_INPUT_BYTES`] (2,000).
    pub fn from_env() -> Self {
        Self::from_env_with(|k| std::env::var(k).ok())
    }

    pub(crate) fn from_env_with(get_env: impl Fn(&str) -> Option<String>) -> Self {
        let parse_positive = |key: &str, default: usize| {
            get_env(key)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|&v| v > 0)
                .unwrap_or(default)
        };
        Self {
            max_batch: parse_positive(EMBED_MAX_BATCH_ENV, DEFAULT_EMBED_MAX_BATCH),
            max_input_bytes: parse_positive(
                EMBED_MAX_INPUT_BYTES_ENV,
                DEFAULT_EMBED_MAX_INPUT_BYTES,
            ),
        }
    }

    /// Truncate `text` to at most `self.max_input_bytes` UTF-8 bytes without
    /// ever cutting inside a multi-byte character.
    pub fn truncate_input<'a>(&self, text: &'a str) -> &'a str {
        truncate_utf8_bytes(text, self.max_input_bytes)
    }

    /// Split `texts` into ordered sub-batches of at most `self.max_batch`
    /// inputs, truncating every input to at most `self.max_input_bytes` UTF-8
    /// bytes on a valid character boundary.
    pub fn prepare_batches<'a>(&self, texts: &'a [String]) -> Vec<Vec<&'a str>> {
        let cap = self.max_batch.max(1);
        texts
            .chunks(cap)
            .map(|chunk| chunk.iter().map(|t| self.truncate_input(t)).collect())
            .collect()
    }
}

/// Truncate `text` to at most `max_bytes` UTF-8 bytes on a character boundary,
/// never splitting a multi-byte UTF-8 character.
pub fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Resolve the maximum inputs per embedding request from `MAKAKOO_EMBED_MAX_BATCH`
/// (default [`DEFAULT_EMBED_MAX_BATCH`]).
pub fn resolve_embed_max_batch() -> usize {
    EmbedRequestLimits::from_env().max_batch
}

/// Resolve the maximum UTF-8 bytes per embedding input from
/// `MAKAKOO_EMBED_MAX_INPUT_BYTES` (default [`DEFAULT_EMBED_MAX_INPUT_BYTES`]).
pub fn resolve_embed_max_input_bytes() -> usize {
    EmbedRequestLimits::from_env().max_input_bytes
}

/// Single text embedding paired with the model id and dim reported by the endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedResult {
    pub vector: Vec<f32>,
    pub model: String,
    pub dim: usize,
}

/// Batch text embeddings paired with the model id and dim reported by the endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedBatchResult {
    pub vectors: Vec<Vec<f32>>,
    pub model: String,
    pub dim: usize,
}

/// Resolve the short-call embedding model from `MAKAKOO_EMBED_MODEL`, falling
/// back to [`DEFAULT_EMBED_MODEL`].
pub fn resolve_short_model() -> String {
    resolve_short_model_with(|k| std::env::var(k).ok())
}

/// Resolve the bulk-indexing embedding model from `MAKAKOO_BULK_EMBED_MODEL`,
/// falling back to `MAKAKOO_EMBED_MODEL`, then [`DEFAULT_EMBED_MODEL`].
pub fn resolve_bulk_model() -> String {
    resolve_bulk_model_with(|k| std::env::var(k).ok())
}

pub(crate) fn resolve_short_model_with(get_env: impl Fn(&str) -> Option<String>) -> String {
    get_env("MAKAKOO_EMBED_MODEL")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string())
}

pub(crate) fn resolve_bulk_model_with(get_env: impl Fn(&str) -> Option<String>) -> String {
    get_env("MAKAKOO_BULK_EMBED_MODEL")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| resolve_short_model_with(get_env))
}

#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    base_url: String,
    api_key: Option<String>,
    model: String,
    timeout: Option<Duration>,
    max_batch: Option<usize>,
    max_input_bytes: Option<usize>,
    client: Arc<OnceCell<reqwest::Client>>,
}

impl Default for EmbeddingClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingClient {
    /// Construct a client for short/interactive embedding calls (`MAKAKOO_EMBED_MODEL`).
    pub fn new() -> Self {
        Self::from_model(resolve_short_model())
    }

    /// Construct a client for bulk indexing (`MAKAKOO_BULK_EMBED_MODEL`, falling
    /// back to `MAKAKOO_EMBED_MODEL`).
    pub fn new_bulk() -> Self {
        Self::from_model(resolve_bulk_model())
    }

    fn from_model(model: String) -> Self {
        let base_url = std::env::var("AIL_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let api_key = std::env::var("AIL_API_KEY").ok().filter(|s| !s.is_empty());
        Self {
            base_url,
            api_key,
            model,
            timeout: None,
            max_batch: None,
            max_input_bytes: None,
            client: Arc::new(OnceCell::new()),
        }
    }

    fn http_client(&self) -> Result<&reqwest::Client> {
        self.client.get_or_try_init(|| {
            reqwest::Client::builder()
                .timeout(DEFAULT_EMBED_TIMEOUT)
                .build()
                .map_err(|e| MakakooError::llm(format!("embedding http client init failed: {e}")))
        })
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut c = Self::new();
        c.base_url = base_url.into();
        c
    }

    pub fn with_endpoint(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Override the per-request maximum batch size for this client (0 is ignored).
    pub fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = (max_batch > 0).then_some(max_batch);
        self
    }

    /// Override the per-input UTF-8 byte cap for this client (0 is ignored).
    pub fn with_max_input_bytes(mut self, max_input_bytes: usize) -> Self {
        self.max_input_bytes = (max_input_bytes > 0).then_some(max_input_bytes);
        self
    }

    /// Effective request limits for this client: explicit client override →
    /// environment override (`MAKAKOO_EMBED_MAX_BATCH` /
    /// `MAKAKOO_EMBED_MAX_INPUT_BYTES`) → defaults (16 inputs / 2,000 bytes).
    pub fn request_limits(&self) -> EmbedRequestLimits {
        let mut limits = EmbedRequestLimits::from_env();
        if let Some(max_batch) = self.max_batch {
            limits.max_batch = max_batch;
        }
        if let Some(max_input_bytes) = self.max_input_bytes {
            limits.max_input_bytes = max_input_bytes;
        }
        limits
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed a single text, returning only the vector.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embed_with_meta(text).await?.vector)
    }

    /// Embed a single text, returning the vector plus endpoint-reported model id and dim.
    pub async fn embed_with_meta(&self, text: &str) -> Result<EmbedResult> {
        let limits = self.request_limits();
        let truncated = limits.truncate_input(text);
        let mut batch = self
            .request_embeddings_chunk(&[truncated], self.timeout)
            .await?;
        let vector = batch
            .vectors
            .pop()
            .ok_or_else(|| MakakooError::llm("embeddings response was empty"))?;
        Ok(EmbedResult {
            dim: vector.len(),
            vector,
            model: batch.model,
        })
    }

    /// Embed a batch of texts, returning only the vectors.
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(self.embed_batch_with_meta(texts).await?.vectors)
    }

    /// Embed a batch of texts, returning vectors plus the model id and dim
    /// reported by the endpoint (falling back to the configured model name if
    /// the endpoint omits `"model"`). Inputs exceeding [`EmbedRequestLimits`]
    /// are split across ordered requests and truncated on UTF-8 boundaries.
    pub async fn embed_batch_with_meta(&self, texts: &[String]) -> Result<EmbedBatchResult> {
        self.request_embeddings(texts, self.timeout).await
    }

    /// Embed a batch of texts with an explicit per-request timeout (overrides
    /// the client's default HTTP timeout for each underlying request).
    pub async fn embed_batch_with_meta_timeout(
        &self,
        texts: &[String],
        timeout: Duration,
    ) -> Result<EmbedBatchResult> {
        self.request_embeddings(texts, Some(timeout)).await
    }

    /// Non-blocking-friendly probe (short timeout) to learn the endpoint's
    /// reported `(model, dim)` without changing any local state on error.
    pub async fn probe_model(&self) -> Result<(String, usize)> {
        self.probe_model_with_timeout(self.timeout.unwrap_or(PROBE_TIMEOUT))
            .await
    }

    /// Probe the endpoint's reported `(model, dim)` with an explicit per-call
    /// timeout.
    pub async fn probe_model_with_timeout(&self, timeout: Duration) -> Result<(String, usize)> {
        let res = self
            .request_embeddings_chunk(&["probe"], Some(timeout))
            .await?;
        if res.dim == 0 {
            return Err(MakakooError::llm(
                "embeddings probe returned zero-dim vector",
            ));
        }
        Ok((res.model, res.dim))
    }

    async fn request_embeddings(
        &self,
        texts: &[String],
        timeout: Option<Duration>,
    ) -> Result<EmbedBatchResult> {
        if texts.is_empty() {
            return Ok(EmbedBatchResult {
                vectors: Vec::new(),
                model: self.model.clone(),
                dim: 0,
            });
        }
        let limits = self.request_limits();
        let batches = limits.prepare_batches(texts);
        let mut all_vectors = Vec::with_capacity(texts.len());
        let mut reported_model: Option<String> = None;
        let mut reported_dim: Option<usize> = None;

        for batch in batches {
            let sub = self.request_embeddings_chunk(&batch, timeout).await?;
            if sub.vectors.is_empty() {
                return Err(MakakooError::llm("embeddings response was empty"));
            }
            if sub.vectors.len() != batch.len() {
                return Err(MakakooError::llm(format!(
                    "embeddings batch expected {} vectors, got {}",
                    batch.len(),
                    sub.vectors.len()
                )));
            }
            if let Some(ref prev_model) = reported_model {
                if prev_model != &sub.model {
                    return Err(MakakooError::llm(format!(
                        "embeddings model changed mid-batch: {prev_model} -> {}",
                        sub.model
                    )));
                }
            } else {
                reported_model = Some(sub.model);
            }
            if let Some(prev_dim) = reported_dim {
                if prev_dim != sub.dim {
                    return Err(MakakooError::llm(
                        "inconsistent embedding dimensions in batch response",
                    ));
                }
            } else {
                reported_dim = Some(sub.dim);
            }
            all_vectors.extend(sub.vectors);
        }

        Ok(EmbedBatchResult {
            vectors: all_vectors,
            model: reported_model.unwrap_or_else(|| self.model.clone()),
            dim: reported_dim.unwrap_or(0),
        })
    }

    async fn request_embeddings_chunk(
        &self,
        texts: &[&str],
        timeout: Option<Duration>,
    ) -> Result<EmbedBatchResult> {
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({
            "model": self.model,
            "input": texts,
        });
        let mut req = self.http_client()?.post(&url).json(&body);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        if let Some(k) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(MakakooError::llm(format!("http {status}: {text}")));
        }
        let v: Value = resp.json().await?;
        let reported_model = v
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.model.clone());
        let data = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| MakakooError::llm("no data array in embeddings response"))?;
        let out = response::order_embedding_items(data)?;
        let dim = out.first().map(|vec| vec.len()).unwrap_or(0);
        if out.iter().any(|vec| vec.len() != dim) {
            return Err(MakakooError::llm(
                "inconsistent embedding dimensions in batch response",
            ));
        }
        Ok(EmbedBatchResult {
            vectors: out,
            model: reported_model,
            dim,
        })
    }
}
