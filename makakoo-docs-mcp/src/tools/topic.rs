// makakoo_docs_topic — Phase C target.
//
// Signature (locked):
//   makakoo_docs_topic(name: str)
//     -> { breadcrumb: [path...], related: [path...], canonical: path }

use anyhow::Result;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::index::Index;

/// Input parameters for `makakoo_docs_topic`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TopicInput {
    pub name: String,
}

/// Result returned by `makakoo_docs_topic`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TopicResult {
    pub breadcrumb: Vec<String>,
    pub related: Vec<String>,
    pub canonical: Option<String>,
}

pub fn run(index: &Index, input: TopicInput) -> Result<TopicResult> {
    index.topic(&input.name)
}
