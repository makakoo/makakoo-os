//! Embeddings client for OpenAI-compatible `/embeddings` endpoints.
//!
//! Default model: `qwen3-embedding:0.6b` — matches the canonical Brain
//! sync model used by the reference install.

use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{MakakooError, Result};
use crate::llm::DEFAULT_BASE_URL;

pub const DEFAULT_EMBED_MODEL: &str = "qwen3-embedding:0.6b";
/// Existing default per-request timeout for embedding HTTP calls.
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(60);
/// Existing default per-request timeout for non-blocking model probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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
    client: reqwest::Client,
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
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_EMBED_TIMEOUT)
            .build()
            .expect("reqwest client build");
        Self {
            base_url,
            api_key,
            model,
            timeout: None,
            client,
        }
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
        let mut batch = self.embed_batch_with_meta(&[text.to_string()]).await?;
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
    /// the endpoint omits `"model"`).
    pub async fn embed_batch_with_meta(&self, texts: &[String]) -> Result<EmbedBatchResult> {
        self.request_embeddings(texts, self.timeout).await
    }

    /// Embed a batch of texts with an explicit per-request timeout (overrides
    /// the client's default HTTP timeout for this call).
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
            .request_embeddings(&["probe".to_string()], Some(timeout))
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
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({
            "model": self.model,
            "input": texts,
        });
        let mut req = self.client.post(&url).json(&body);
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
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| MakakooError::llm("missing embedding array"))?;
            let vec: Vec<f32> = emb
                .iter()
                .filter_map(|n| n.as_f64().map(|f| f as f32))
                .collect();
            out.push(vec);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn default_model_is_qwen3() {
        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            resolve_short_model_with(|k| empty.get(k).map(|v| (*v).to_string())),
            DEFAULT_EMBED_MODEL
        );
        assert_eq!(
            resolve_bulk_model_with(|k| empty.get(k).map(|v| (*v).to_string())),
            DEFAULT_EMBED_MODEL
        );
    }

    #[test]
    fn bulk_and_short_env_selection_precedence() {
        let mut env = HashMap::new();
        env.insert("MAKAKOO_EMBED_MODEL", "short-alias");
        let get = |k: &str| env.get(k).map(|v| (*v).to_string());
        assert_eq!(resolve_short_model_with(get), "short-alias");
        assert_eq!(resolve_bulk_model_with(get), "short-alias");

        env.insert("MAKAKOO_BULK_EMBED_MODEL", "  ");
        let get = |k: &str| env.get(k).map(|v| (*v).to_string());
        assert_eq!(resolve_bulk_model_with(get), "short-alias");

        env.insert("MAKAKOO_BULK_EMBED_MODEL", "bulk-alias");
        let get = |k: &str| env.get(k).map(|v| (*v).to_string());
        assert_eq!(resolve_short_model_with(get), "short-alias");
        assert_eq!(resolve_bulk_model_with(get), "bulk-alias");
    }

    #[test]
    fn with_model_overrides() {
        let c = EmbeddingClient::new().with_model("custom-model");
        assert_eq!(c.model(), "custom-model");
    }

    #[tokio::test]
    async fn embed_single_text_captures_reported_model_and_dim() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "Qwen3-Embedding-0.6B",
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] }
                ]
            })))
            .mount(&server)
            .await;

        let client = EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed");
        let res = client.embed_with_meta("hello world").await.unwrap();
        assert_eq!(res.model, "Qwen3-Embedding-0.6B");
        assert_eq!(res.dim, 3);
        assert_eq!(res.vector.len(), 3);
        assert!((res.vector[0] - 0.1).abs() < 1e-5);
    }

    #[tokio::test]
    async fn embed_falls_back_to_configured_model_when_response_omits_or_empties_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "   ",
                "data": [
                    { "embedding": [1.0, 2.0] },
                    { "embedding": [3.0, 4.0] }
                ]
            })))
            .mount(&server)
            .await;

        let client = EmbeddingClient::with_base_url(server.uri()).with_model("configured-fallback");
        let res = client
            .embed_batch_with_meta(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(res.model, "configured-fallback");
        assert_eq!(res.dim, 2);
        assert_eq!(res.vectors.len(), 2);
        assert_eq!(res.vectors[0], vec![1.0, 2.0]);
        assert_eq!(res.vectors[1], vec![3.0, 4.0]);
    }
}
