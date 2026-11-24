//! Guard tests for OpenAI `/embeddings` response `data[].index` permutation
//! validation (review r3 MED): exact permutation of `0..n` required when any
//! index is present; duplicates, gaps, out-of-range, non-integer, or mixed
//! indexed/unindexed items return `MakakooError::Llm`; all-omitted indices
//! preserve arrival order.

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

async fn embed_with_raw_data(data: Value, inputs: &[&str]) -> Result<EmbedBatchResult> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "test-model",
            "data": data,
        })))
        .mount(&server)
        .await;
    let client = EmbeddingClient::with_base_url(server.uri())
        .with_model("ail-embed")
        .with_max_batch(16)
        .with_max_input_bytes(2_000);
    let texts: Vec<String> = inputs.iter().map(|s| (*s).to_string()).collect();
    client.embed_batch_with_meta(&texts).await
}

#[tokio::test]
async fn embed_batch_reorders_valid_shuffled_index_permutation() {
    let res = embed_with_raw_data(
        json!([
            { "index": 2, "embedding": [20.0, 1.0] },
            { "index": 0, "embedding": [0.0, 1.0] },
            { "index": 1, "embedding": [10.0, 1.0] },
        ]),
        &["a", "b", "c"],
    )
    .await
    .unwrap();
    assert_eq!(
        res.vectors,
        vec![vec![0.0, 1.0], vec![10.0, 1.0], vec![20.0, 1.0]]
    );
}

#[tokio::test]
async fn embed_batch_preserves_arrival_order_when_no_indices_present() {
    let res = embed_with_raw_data(
        json!([
            { "embedding": [10.0, 1.0] },
            { "embedding": [20.0, 1.0] },
            { "embedding": [30.0, 1.0] },
        ]),
        &["a", "b", "c"],
    )
    .await
    .unwrap();
    assert_eq!(
        res.vectors,
        vec![vec![10.0, 1.0], vec![20.0, 1.0], vec![30.0, 1.0]]
    );
}

#[tokio::test]
async fn embed_batch_rejects_duplicate_response_indices() {
    for data in [
        json!([
            { "index": 0, "embedding": [1.0, 0.0] },
            { "index": 0, "embedding": [2.0, 0.0] },
            { "index": 1, "embedding": [3.0, 0.0] },
        ]),
        json!([
            { "index": 0, "embedding": [1.0, 0.0] },
            { "index": 0, "embedding": [2.0, 0.0] },
            { "index": 2, "embedding": [3.0, 0.0] },
        ]),
    ] {
        let err = embed_with_raw_data(data, &["a", "b", "c"])
            .await
            .unwrap_err();
        assert!(
            matches!(err, MakakooError::Llm(ref msg) if msg.contains("duplicate index")),
            "expected duplicate index Llm error, got: {err:?}"
        );
    }
}

#[tokio::test]
async fn embed_batch_rejects_gap_and_out_of_range_response_indices() {
    let cases: &[(&str, Value, &[&str])] = &[
        (
            "gap in 2-item response [0, 2]",
            json!([
                { "index": 0, "embedding": [1.0, 0.0] },
                { "index": 2, "embedding": [2.0, 0.0] },
            ]),
            &["a", "b"],
        ),
        (
            "gap in 3-item response [0, 2, 3]",
            json!([
                { "index": 0, "embedding": [1.0, 0.0] },
                { "index": 2, "embedding": [2.0, 0.0] },
                { "index": 3, "embedding": [3.0, 0.0] },
            ]),
            &["a", "b", "c"],
        ),
        (
            "1-based out-of-range [1, 2, 3]",
            json!([
                { "index": 1, "embedding": [1.0, 0.0] },
                { "index": 2, "embedding": [2.0, 0.0] },
                { "index": 3, "embedding": [3.0, 0.0] },
            ]),
            &["a", "b", "c"],
        ),
        (
            "negative out-of-range [-1, 0, 1]",
            json!([
                { "index": -1, "embedding": [1.0, 0.0] },
                { "index": 0, "embedding": [2.0, 0.0] },
                { "index": 1, "embedding": [3.0, 0.0] },
            ]),
            &["a", "b", "c"],
        ),
        (
            "non-integer index [\"0\", \"1\"]",
            json!([
                { "index": "0", "embedding": [1.0, 0.0] },
                { "index": "1", "embedding": [2.0, 0.0] },
            ]),
            &["a", "b"],
        ),
    ];
    for (label, data, inputs) in cases {
        let err = embed_with_raw_data(data.clone(), inputs).await.unwrap_err();
        assert!(
            matches!(
                err,
                MakakooError::Llm(ref msg)
                    if msg.contains("out of range") || msg.contains("invalid index")
            ),
            "{label}: expected out-of-range/invalid index Llm error, got: {err:?}"
        );
    }
}

#[tokio::test]
async fn embed_batch_rejects_mixed_indexed_and_unindexed_items() {
    for (data, inputs) in [
        (
            json!([
                { "index": 0, "embedding": [1.0, 0.0] },
                { "embedding": [2.0, 0.0] },
                { "index": 2, "embedding": [3.0, 0.0] },
            ]),
            &["a", "b", "c"][..],
        ),
        (
            json!([
                { "embedding": [1.0, 0.0] },
                { "index": 1, "embedding": [2.0, 0.0] },
            ]),
            &["a", "b"][..],
        ),
        (
            json!([
                { "index": 0, "embedding": [1.0, 0.0] },
                { "index": null, "embedding": [2.0, 0.0] },
            ]),
            &["a", "b"][..],
        ),
    ] {
        let err = embed_with_raw_data(data, inputs).await.unwrap_err();
        assert!(
            matches!(
                err,
                MakakooError::Llm(ref msg) if msg.contains("mixes indexed and unindexed items")
            ),
            "expected mixed-index Llm error, got: {err:?}"
        );
    }
}
