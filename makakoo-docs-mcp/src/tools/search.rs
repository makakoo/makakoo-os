// makakoo_docs_search — full-text search across the indexed corpus.
//
// Signature (locked):
//   makakoo_docs_search(query: str, limit?: int = 10)
//     -> list[{ path, title, snippet, score }]

use anyhow::Result;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::index::Index;

/// Input parameters for `makakoo_docs_search`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchInput {
    pub query: String,
    pub limit: Option<usize>,
}

/// One search result returned by `makakoo_docs_search`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
}

pub fn run(index: &Index, input: SearchInput) -> Result<Vec<SearchHit>> {
    let limit = input.limit.unwrap_or(10).clamp(1, 50);
    index.search(&input.query, limit)
}
