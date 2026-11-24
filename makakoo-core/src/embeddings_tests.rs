//! Unit and guard tests for [`crate::embeddings`] — model env precedence,
//! batch splitting (`MAKAKOO_EMBED_MAX_BATCH`, default 16), and UTF-8 byte
//! truncation (`MAKAKOO_EMBED_MAX_INPUT_BYTES`, default 2,000).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;

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

#[test]
fn embed_request_limits_defaults_and_env_overrides() {
    assert_eq!(DEFAULT_EMBED_MAX_BATCH, 16);
    assert_eq!(DEFAULT_EMBED_MAX_INPUT_BYTES, 2_000);

    let defaults = EmbedRequestLimits::from_env_with(|_| None);
    assert_eq!(defaults.max_batch, 16);
    assert_eq!(defaults.max_input_bytes, 2_000);

    let valid = EmbedRequestLimits::from_env_with(|k| match k {
        EMBED_MAX_BATCH_ENV => Some(" 8 ".to_string()),
        EMBED_MAX_INPUT_BYTES_ENV => Some("1500".to_string()),
        _ => None,
    });
    assert_eq!(valid.max_batch, 8);
    assert_eq!(valid.max_input_bytes, 1500);

    for bad in ["", "  ", "0", "-4", "abc"] {
        let fallback = EmbedRequestLimits::from_env_with(|k| match k {
            EMBED_MAX_BATCH_ENV | EMBED_MAX_INPUT_BYTES_ENV => Some(bad.to_string()),
            _ => None,
        });
        assert_eq!(
            fallback, defaults,
            "invalid env value {bad:?} must fall back to defaults"
        );
    }
}

#[test]
fn truncate_utf8_bytes_cuts_on_char_boundary_for_polish_german_and_multibyte() {
    // 2,000 Polish characters (> 2,000 UTF-8 bytes because of 2-byte diacritics).
    let polish: String = "Zażółć gęślą jaźń — ą ć ę ł ń ó ś ź ż. "
        .chars()
        .cycle()
        .take(2_000)
        .collect();
    assert_eq!(polish.chars().count(), 2_000);
    assert!(
        polish.len() > DEFAULT_EMBED_MAX_INPUT_BYTES,
        "2,000 Polish chars must exceed 2,000 UTF-8 bytes"
    );
    let cut_pl = truncate_utf8_bytes(&polish, DEFAULT_EMBED_MAX_INPUT_BYTES);
    assert!(cut_pl.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    assert!(polish.starts_with(cut_pl));
    let next_pl = polish[cut_pl.len()..].chars().next().unwrap();
    assert!(cut_pl.len() + next_pl.len_utf8() > DEFAULT_EMBED_MAX_INPUT_BYTES);

    // 2,000 German characters (> 2,000 UTF-8 bytes because of ä ö ü ß).
    let german: String = "Prüfung über größere Änderungen für Maßstäbe — Ärger, Öl, Übung. "
        .chars()
        .cycle()
        .take(2_000)
        .collect();
    assert_eq!(german.chars().count(), 2_000);
    assert!(german.len() > DEFAULT_EMBED_MAX_INPUT_BYTES);
    let cut_de = truncate_utf8_bytes(&german, DEFAULT_EMBED_MAX_INPUT_BYTES);
    assert!(cut_de.len() <= DEFAULT_EMBED_MAX_INPUT_BYTES);
    assert!(german.starts_with(cut_de));
    let next_de = german[cut_de.len()..].chars().next().unwrap();
    assert!(cut_de.len() + next_de.len_utf8() > DEFAULT_EMBED_MAX_INPUT_BYTES);

    // Exact straddling boundaries for 2-byte, 3-byte, and 4-byte scalars.
    let two_byte_straddle = format!("{}łtail", "a".repeat(1_999));
    assert_eq!(truncate_utf8_bytes(&two_byte_straddle, 2_000).len(), 1_999);

    let three_byte_straddle = format!("{}€tail", "a".repeat(1_998));
    assert_eq!(
        truncate_utf8_bytes(&three_byte_straddle, 2_000).len(),
        1_998
    );

    let four_byte_straddle = format!("{}🚀tail", "a".repeat(1_997));
    assert_eq!(truncate_utf8_bytes(&four_byte_straddle, 2_000).len(), 1_997);

    assert_eq!(truncate_utf8_bytes("ł", 1), "");
    assert_eq!(truncate_utf8_bytes("abc", 0), "");
    assert_eq!(truncate_utf8_bytes("short", 2_000), "short");
}

#[derive(Clone)]
struct StrictLimitResponder {
    max_inputs: usize,
    max_bytes: usize,
    model: String,
    batch_sizes: Arc<Mutex<Vec<usize>>>,
    received_inputs: Arc<Mutex<Vec<String>>>,
}

impl StrictLimitResponder {
    fn new(max_inputs: usize, max_bytes: usize, model: &str) -> Self {
        Self {
            max_inputs,
            max_bytes,
            model: model.to_string(),
            batch_sizes: Arc::new(Mutex::new(Vec::new())),
            received_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Respond for StrictLimitResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let arr = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if arr.len() > self.max_inputs {
            return ResponseTemplate::new(400).set_body_json(json!({
                "error": format!("too many inputs: {} > {}", arr.len(), self.max_inputs)
            }));
        }
        let mut data = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let text = item.as_str().unwrap_or("");
            if text.len() > self.max_bytes {
                return ResponseTemplate::new(400).set_body_json(json!({
                    "error": format!("input byte len {} > {}", text.len(), self.max_bytes)
                }));
            }
            self.received_inputs.lock().unwrap().push(text.to_string());
            // Encode leading integer tag (when present) into vector[0] so callers
            // can prove exact input ordering across split requests.
            let tag = text
                .split(':')
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(i as f32);
            data.push(json!({
                "index": i,
                "embedding": [tag, text.len() as f32, 1.0]
            }));
        }
        self.batch_sizes.lock().unwrap().push(arr.len());
        ResponseTemplate::new(200).set_body_json(json!({
            "model": self.model,
            "data": data,
        }))
    }
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

#[tokio::test]
async fn embed_batch_splits_over_16_inputs_preserves_order_and_truncates_utf8_bytes() {
    let responder = StrictLimitResponder::new(16, 2_000, "strict-embed-v1");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let pl_tail: String = "Zażółć gęślą jaźń ".chars().cycle().take(2_000).collect();
    let de_tail: String = "Prüfung größere Maßstäbe "
        .chars()
        .cycle()
        .take(2_000)
        .collect();
    let texts: Vec<String> = (0..35)
        .map(|i| match i % 3 {
            0 => format!("{i}:{pl_tail}"),
            1 => format!("{i}:{de_tail}"),
            _ => format!("{i}:short entry"),
        })
        .collect();

    let client = EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed");
    let res = client.embed_batch_with_meta(&texts).await.unwrap();

    assert_eq!(res.model, "strict-embed-v1");
    assert_eq!(res.dim, 3);
    assert_eq!(res.vectors.len(), 35);
    assert_eq!(
        responder.batch_sizes.lock().unwrap().as_slice(),
        &[16, 16, 3]
    );
    for (i, vec) in res.vectors.iter().enumerate() {
        assert_eq!(vec[0], i as f32, "vector at index {i} out of order");
    }
    let received = responder.received_inputs.lock().unwrap();
    assert_eq!(received.len(), 35);
    for (orig, sent) in texts.iter().zip(received.iter()) {
        assert!(sent.len() <= 2_000, "sent {} bytes (> 2000)", sent.len());
        assert!(orig.starts_with(sent.as_str()));
    }
}

#[tokio::test]
async fn embed_single_query_truncates_multibyte_text_to_byte_limit() {
    let responder = StrictLimitResponder::new(16, 2_000, "strict-embed-v1");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let long_query: String = "42:Prüfung zażółć gęślą größere "
        .chars()
        .cycle()
        .take(2_000)
        .collect();
    assert!(long_query.len() > 2_000);

    let client = EmbeddingClient::with_base_url(server.uri()).with_model("ail-embed");
    let res = client.embed_with_meta(&long_query).await.unwrap();
    assert_eq!(res.vector[0], 42.0);
    let sent = &responder.received_inputs.lock().unwrap()[0];
    assert!(sent.len() <= 2_000);
    assert!(long_query.starts_with(sent.as_str()));
}

#[tokio::test]
async fn embed_client_builder_overrides_cap_batch_and_input_bytes() {
    let responder = StrictLimitResponder::new(5, 80, "strict-override");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let texts: Vec<String> = (0..12)
        .map(|i| format!("{i}:{}", "łóżko ".repeat(30)))
        .collect();
    let client = EmbeddingClient::with_base_url(server.uri())
        .with_model("ail-embed")
        .with_max_batch(5)
        .with_max_input_bytes(80);
    let res = client.embed_batch_with_meta(&texts).await.unwrap();

    assert_eq!(res.vectors.len(), 12);
    assert_eq!(responder.batch_sizes.lock().unwrap().as_slice(), &[5, 5, 2]);
    for (i, vec) in res.vectors.iter().enumerate() {
        assert_eq!(vec[0], i as f32);
    }
    for sent in responder.received_inputs.lock().unwrap().iter() {
        assert!(sent.len() <= 80);
    }
}

#[derive(Clone)]
struct FlippingModelResponder {
    calls: Arc<AtomicUsize>,
}

impl Respond for FlippingModelResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let inputs = body
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(1);
        let model = if n == 0 { "model-a" } else { "model-b" };
        let data: Vec<Value> = (0..inputs)
            .map(|_| json!({ "embedding": [1.0, 0.0] }))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "model": model, "data": data }))
    }
}

#[tokio::test]
async fn embed_batch_errors_when_model_changes_across_split_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(FlippingModelResponder {
            calls: Arc::new(AtomicUsize::new(0)),
        })
        .mount(&server)
        .await;

    let texts: Vec<String> = (0..5).map(|i| format!("doc {i}")).collect();
    let client = EmbeddingClient::with_base_url(server.uri())
        .with_model("ail-embed")
        .with_max_batch(3);
    let err = client.embed_batch_with_meta(&texts).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("embeddings model changed mid-batch"),
        "unexpected error: {err}"
    );
}
