// makakoo_docs_read — Phase C target.
//
// Signature (locked):
//   makakoo_docs_read(path: str) -> string (full markdown)

use anyhow::Result;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

use crate::index::Index;

/// Input parameters for `makakoo_docs_read`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadInput {
    pub path: String,
}

pub fn run(index: &Index, input: ReadInput) -> Result<Option<String>> {
    index.read(&input.path)
}
