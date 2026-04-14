//! Embeddings client for switchAILocal.
//!
//! Default model: `qwen3-embedding:0.6b` — matches the canonical Brain
//! sync model used by the reference install.

use serde_json::{Value, json};

use crate::error::{MakakooError, Result};
use crate::llm::DEFAULT_BASE_URL;

pub const DEFAULT_EMBED_MODEL: &str = "qwen3-embedding:0.6b";

#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    base_url: String,
    api_key: Option<String>,
    model: String,
    client: reqwest::Client,
}

impl Default for EmbeddingClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingClient {
    pub fn new() -> Self {
        let base_url = std::env::var("AIL_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let api_key = std::env::var("AIL_API_KEY").ok().filter(|s| !s.is_empty());
        let model = std::env::var("MAKAKOO_EMBED_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string());
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client build");
        Self {
            base_url,
            api_key,
            model,
            client,
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut c = Self::new();
        c.base_url = base_url.into();
        c
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed a single text, returning the vector.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut vecs = self.embed_batch(&[text.to_string()]).await?;
        vecs.pop()
            .ok_or_else(|| MakakooError::llm("embeddings response was empty"))
    }

    /// Embed a batch of texts.
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({
            "model": self.model,
            "input": texts,
        });
        let mut req = self.client.post(&url).json(&body);
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
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn default_model_is_qwen3() {
        // Ensure test isolation from inherited env.
        std::env::remove_var("MAKAKOO_EMBED_MODEL");
        let c = EmbeddingClient::new();
        assert_eq!(c.model(), DEFAULT_EMBED_MODEL);
    }

    #[test]
    fn with_model_overrides() {
        let c = EmbeddingClient::new().with_model("custom-model");
        assert_eq!(c.model(), "custom-model");
    }

    #[tokio::test]
    async fn embed_single_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": [0.1, 0.2, 0.3] }
                ]
            })))
            .mount(&server)
            .await;

        let client = EmbeddingClient::with_base_url(server.uri());
        let v = client.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 3);
        assert!((v[0] - 0.1).abs() < 1e-5);
    }

    #[tokio::test]
    async fn embed_batch_roundtrip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": [1.0, 2.0] },
                    { "embedding": [3.0, 4.0] }
                ]
            })))
            .mount(&server)
            .await;

        let client = EmbeddingClient::with_base_url(server.uri());
        let vs = client
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(vs.len(), 2);
        assert_eq!(vs[0], vec![1.0, 2.0]);
        assert_eq!(vs[1], vec![3.0, 4.0]);
    }
}
