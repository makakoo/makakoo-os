//! OpenAI-compatible `/embeddings` response parsing and strict index-permutation
//! validation.

use serde_json::Value;

use crate::error::{MakakooError, Result};

/// Extract vectors from an OpenAI-compatible `data` array.
///
/// If no item carries an `index`, vectors are returned in arrival order.
/// Otherwise every item must carry a non-negative integer `index` forming an
/// exact permutation of `0..data.len()`; duplicates, gaps, out-of-range or
/// non-integer values, or a mix of indexed and unindexed items return an error.
pub(crate) fn order_embedding_items(data: &[Value]) -> Result<Vec<Vec<f32>>> {
    let n = data.len();
    let mut parsed: Vec<(Option<usize>, Vec<f32>)> = Vec::with_capacity(n);
    let mut indexed_count = 0usize;
    for item in data {
        let emb = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| MakakooError::llm("missing embedding array"))?;
        let vec: Vec<f32> = emb
            .iter()
            .filter_map(|n| n.as_f64().map(|f| f as f32))
            .collect();
        let idx = match item.get("index") {
            None | Some(Value::Null) => None,
            Some(raw) => {
                let i = raw
                    .as_u64()
                    .and_then(|u| usize::try_from(u).ok())
                    .ok_or_else(|| {
                        MakakooError::llm(format!("embeddings response has invalid index: {raw}"))
                    })?;
                indexed_count += 1;
                Some(i)
            }
        };
        parsed.push((idx, vec));
    }
    if indexed_count == 0 {
        return Ok(parsed.into_iter().map(|(_, vec)| vec).collect());
    }
    if indexed_count != n {
        return Err(MakakooError::llm(
            "embeddings response mixes indexed and unindexed items",
        ));
    }
    let mut ordered: Vec<Option<Vec<f32>>> = vec![None; n];
    for (idx, vec) in parsed {
        let idx = idx.ok_or_else(|| {
            MakakooError::llm("embeddings response mixes indexed and unindexed items")
        })?;
        if idx >= n {
            return Err(MakakooError::llm(format!(
                "embeddings response index {idx} out of range for {n} items"
            )));
        }
        if ordered[idx].is_some() {
            return Err(MakakooError::llm(format!(
                "embeddings response contains duplicate index {idx}"
            )));
        }
        ordered[idx] = Some(vec);
    }
    ordered
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            slot.ok_or_else(|| {
                MakakooError::llm(format!(
                    "embeddings response missing index {i} for {n} items"
                ))
            })
        })
        .collect()
}
