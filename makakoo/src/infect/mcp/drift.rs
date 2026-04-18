//! Drift detection + repair for the broader infect surface.
//!
//! Implementation lands in Phase D — this stub keeps the module tree
//! buildable while Phase A scaffolds the rest.

#![allow(dead_code)]

use crate::infect::mcp::McpTarget;

/// Per-target drift status. Every field defaults to `false` (clean).
#[derive(Debug, Default, Clone)]
pub struct DriftReport {
    pub target: Option<McpTarget>,
    pub mcp_missing: bool,
    pub mcp_stale_path: bool,
    pub mcp_stale_env: bool,
    pub bootstrap_missing: bool,
    pub memory_broken: bool,
    pub memory_wrong_target: bool,
    pub skills_broken: bool,
    pub skills_wrong_target: bool,
    pub recursive_symlink_in_memory: bool,
}

impl DriftReport {
    pub fn is_clean(&self) -> bool {
        !self.mcp_missing
            && !self.mcp_stale_path
            && !self.mcp_stale_env
            && !self.bootstrap_missing
            && !self.memory_broken
            && !self.memory_wrong_target
            && !self.skills_broken
            && !self.skills_wrong_target
            && !self.recursive_symlink_in_memory
    }
}
