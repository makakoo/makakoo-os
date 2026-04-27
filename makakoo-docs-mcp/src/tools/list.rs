// makakoo_docs_list — Phase C target.
//
// Signature (locked):
//   makakoo_docs_list(prefix?: str) -> list[{ path, size_bytes, title }]

use anyhow::Result;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::index::Index;

/// Input parameters for `makakoo_docs_list`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListInput {
    pub prefix: Option<String>,
}

/// One entry returned by `makakoo_docs_list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListEntry {
    pub path: String,
    pub size_bytes: i64,
    pub title: String,
}

pub fn run(index: &Index, input: ListInput) -> Result<Vec<ListEntry>> {
    index.list(input.prefix.as_deref())
}
