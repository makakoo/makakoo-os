//! Codex TOML adapter — `[mcp_servers.<name>]` inline-table format.
//!
//! Filled in Phase C. Phase A stub returns Skipped so the scaffold
//! builds and CLI dispatch works for JSON-only targets.

use std::path::Path;

use crate::infect::mcp::{McpServerSpec, SyncOutcome};

pub fn sync(_path: &Path, _spec: &McpServerSpec, _dry_run: bool) -> SyncOutcome {
    SyncOutcome::Skipped {
        reason: "codex adapter pending Phase C".to_string(),
    }
}
