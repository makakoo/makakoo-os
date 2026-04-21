//! Tool handler registration.
//!
//! Each MCP tool is a type that implements
//! [`crate::dispatch::ToolHandler`]. This module wires them into the
//! registry at server boot.
//!
//! Handler modules are added by Wave 4 parallel agents:
//!   - `tier_a` — 20 read tools (T13)
//!   - `tier_b` — 15 write tools (T14)
//!   - `tier_c` — 6 heavy tools + swarm gateway (T15)
//!
//! At T12 this module is intentionally empty — the server spine compiles
//! clean with zero tools registered, `--health` reports
//! `{"ok":true,"tools":0}`, and T13/T14/T15 add their registration calls
//! here without needing to touch anything else in the crate.

use crate::dispatch::{ToolContext, ToolRegistry};
use std::sync::Arc;

pub mod tier_a; // T13 — 20 read tools
pub mod tier_b; // T14 — 15 write + multimodal tools
pub mod tier_c; // T15 — 6 heavy + swarm tools

/// Register every tool handler with the shared registry.
///
/// Call once at boot, after `ToolContext` is fully constructed. Wave 4
/// agents append registrations here, one per tier:
///
/// ```ignore
/// tier_a::register_tier_a(registry, ctx.clone());
/// tier_b::register_tier_b(registry, ctx.clone());
/// tier_c::register_tier_c(registry, ctx);
/// ```
pub fn register_all(registry: &mut ToolRegistry, ctx: &Arc<ToolContext>) {
    // T13 — Tier-A: 20 read-only handlers.
    tier_a::register_tier_a(registry, Arc::clone(ctx));

    // T14 — Tier-B: 15 write / mutation + 4 multimodal handlers.
    tier_b::register_tier_b(registry, Arc::clone(ctx));

    // T15 — Tier-C: 6 heavy / swarm / comm handlers.
    tier_c::register_tier_c(registry, Arc::clone(ctx));
}

#[cfg(test)]
mod handler_contract_tests {
    //! v0.2 E.3 — Cross-handler contract enforcement.
    //!
    //! Rather than asserting an exact tool count (which would break every
    //! time someone legitimately adds a handler), these tests enforce
    //! structural invariants every tool must satisfy:
    //!   * unique name
    //!   * non-empty description (agents need to know what the tool does)
    //!   * input_schema is a JSON object (required by MCP spec)
    //!   * name uses lowercase alphanumeric + underscores only
    //!   * tools/list output is stable-sorted across runs
    //!
    //! New handlers added by future sprints automatically pick these up —
    //! no need to touch this file.

    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn registry_with_all() -> (ToolRegistry, Arc<ToolContext>) {
        let ctx = Arc::new(ToolContext::empty(PathBuf::from("/tmp/makakoo-contract-test")));
        let mut reg = ToolRegistry::new();
        register_all(&mut reg, &ctx);
        (reg, ctx)
    }

    #[test]
    fn every_handler_has_unique_name() {
        let (reg, _) = registry_with_all();
        let list = reg.list();
        let mut seen: HashSet<&str> = HashSet::new();
        for d in &list {
            assert!(
                seen.insert(d.name.as_str()),
                "duplicate handler name: {}",
                d.name
            );
        }
    }

    #[test]
    fn every_handler_has_non_empty_description() {
        let (reg, _) = registry_with_all();
        for d in reg.list() {
            assert!(
                !d.description.trim().is_empty(),
                "handler {} has empty description",
                d.name
            );
            assert!(
                d.description.len() >= 20,
                "handler {} description is too short (<20 chars) — agents won't know when to use it: {:?}",
                d.name,
                d.description
            );
        }
    }

    #[test]
    fn every_input_schema_is_a_json_object() {
        let (reg, _) = registry_with_all();
        for d in reg.list() {
            assert!(
                d.input_schema.is_object(),
                "handler {} input_schema is not an object",
                d.name
            );
            let obj = d.input_schema.as_object().unwrap();
            // MCP spec: inputSchema must declare its type
            let ty = obj.get("type").and_then(|v| v.as_str());
            assert_eq!(
                ty,
                Some("object"),
                "handler {} input_schema.type must be \"object\" (got {:?})",
                d.name,
                ty
            );
        }
    }

    #[test]
    fn tool_names_follow_naming_convention() {
        let (reg, _) = registry_with_all();
        for d in reg.list() {
            assert!(
                d.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "handler name {:?} must be lowercase alphanumeric + underscores",
                d.name
            );
            assert!(
                !d.name.starts_with('_'),
                "handler name {:?} starts with an underscore",
                d.name
            );
            assert!(
                d.name.len() >= 3 && d.name.len() <= 64,
                "handler name {:?} length {} outside [3, 64]",
                d.name,
                d.name.len()
            );
        }
    }

    #[test]
    fn tools_list_is_stable_across_calls() {
        let (reg, _) = registry_with_all();
        let a = reg.list();
        let b = reg.list();
        // Stable sort by name means two calls return the same sequence.
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.name, y.name);
        }
    }

    #[test]
    fn tier_counts_match_sprint_manifest() {
        // v0.2: tier_b added 6 pi_* handlers (B.3 + B.4).
        // This test keeps the published count honest. If you add a tool,
        // bump the expected min here so stale docs get caught.
        let (reg, _) = registry_with_all();
        let count = reg.list().len();
        assert!(
            count >= 30,
            "expected ≥30 registered tools (was {}). Did a handler module stop registering?",
            count
        );
    }

    #[test]
    fn pi_handlers_are_registered() {
        // v0.2 B.3+B.4 regression guard — the six pi_* tools must survive
        // every refactor of register_tier_b.
        let (reg, _) = registry_with_all();
        let names: HashSet<String> = reg.list().into_iter().map(|d| d.name).collect();
        for expected in [
            "pi_run",
            "pi_session_fork",
            "pi_session_label",
            "pi_session_export",
            "pi_set_model",
            "pi_steer",
        ] {
            assert!(
                names.contains(expected),
                "missing pi handler: {expected}"
            );
        }
    }
}
