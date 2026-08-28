//! Canonical list of tool names an AgentSpec may reference.
//!
//! Why this exists: `agent create` used to accept any string in a spec's
//! `tools:` list. A typo, or a name the model invented, produced a slot that
//! looked fine and then silently could not do the thing it was created for —
//! `scoped_tools` filters the registry down to the intersection, so an
//! unknown name simply resolves to nothing. Failing at create time turns a
//! silent capability hole into an error message.
//!
//! Slots may name **any** registered MCP tool. There is no separate
//! "agent-safe" subset: `check_tool` is the only gate, and the docs show
//! slots scoping `gmail` and `linkedin`. So this list must track the full
//! registry, and the drift test in makakoo-mcp
//! (`catalog_matches_the_registered_tool_names`) fails CI if a handler is
//! added without an entry here.
//!
//! Pattern tools are deliberately absent: `register_pattern_tools` walks
//! `$MAKAKOO_HOME/plugins/pattern-*/` at boot, so the set is per-machine and
//! cannot be a compile-time constant. [`is_known_tool`] accepts them by
//! prefix instead — rejecting a pattern the user has installed would be worse
//! than accepting one they have not.

/// Every statically registered MCP tool, sorted. Keep sorted: the drift test
/// compares against a sorted registry listing.
pub const TOOL_CATALOG: &[&str] = &[
    "agent_create",
    "agent_info",
    "agent_install",
    "agent_list",
    "agent_uninstall",
    "brain_context",
    "brain_entities",
    "brain_query",
    "brain_recent",
    "brain_search",
    "brain_write_journal",
    "buddy_status",
    "channel_approval_request",
    "channel_directory_list_channels",
    "channel_directory_list_users",
    "channel_directory_lookup_user",
    "channel_messaging_broadcast",
    "channel_messaging_send_channel",
    "channel_messaging_send_dm",
    "channel_threading_create_thread",
    "channel_threading_follow_thread",
    "channel_threading_list_threads",
    "chat_history",
    "chat_send",
    "chat_stats",
    "chat_status",
    "costs_summary",
    "dream",
    "grant_write_access",
    "harvey_brain_search",
    "harvey_brain_write",
    "harvey_browse",
    "harvey_describe_audio",
    "harvey_describe_image",
    "harvey_describe_video",
    "harvey_generate_image",
    "harvey_infect_local",
    "harvey_journal_entry",
    "harvey_knowledge_ingest",
    "harvey_olibia_speak",
    "harvey_superbrain_query",
    "harvey_superbrain_vector_search",
    "harvey_swarm_run",
    "harvey_swarm_status",
    "harvey_telegram_send",
    "list_write_grants",
    "nursery_hatch",
    "nursery_status",
    "outbound_draft",
    "pi_run",
    "pi_session_export",
    "pi_session_fork",
    "pi_session_label",
    "pi_set_model",
    "pi_steer",
    "revoke_write_access",
    "sancho_status",
    "sancho_tick",
    "skill_discover",
    "swarm",
    "wiki_compile",
    "wiki_lint",
    "wiki_save",
    "write_file",
];

/// Prefix for per-machine pattern tools registered from
/// `$MAKAKOO_HOME/plugins/pattern-*/`.
pub const PATTERN_TOOL_PREFIX: &str = "pattern_";

/// The MCP client spelling of a Makakoo tool. AgentSpec accepts either.
const MCP_PREFIX: &str = "mcp__harvey__";

/// Names offered by the interactive `agent create` wizard.
///
/// A deliberately small, safe starting set — not a statement about what a
/// slot *may* use. It previously offered `web_search`, which has never been
/// registered: every agent created through the wizard's default answer got a
/// tool that did not exist.
pub const WIZARD_TOOL_CHOICES: &[&str] = &["brain_search", "brain_recent", "write_file"];

/// Is `name` a tool an AgentSpec may legitimately reference?
///
/// Accepts both the bare Makakoo name and the `mcp__harvey__` spelling, the
/// same equivalence `check_tool` enforces at dispatch — validation that
/// disagreed with enforcement would reject specs that work.
pub fn is_known_tool(name: &str) -> bool {
    let bare = name.strip_prefix(MCP_PREFIX).unwrap_or(name);
    if bare.starts_with(PATTERN_TOOL_PREFIX) {
        return true;
    }
    TOOL_CATALOG.contains(&bare)
}

/// Tool names in `tools` that no registered handler can serve.
pub fn unknown_tools<'a, I>(tools: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    tools
        .into_iter()
        .filter(|name| !is_known_tool(name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_sorted_and_free_of_duplicates() {
        // The drift test compares against a sorted registry listing, so an
        // out-of-order entry would fail there with a confusing diff.
        let mut sorted = TOOL_CATALOG.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            TOOL_CATALOG,
            sorted.as_slice(),
            "TOOL_CATALOG must be sorted"
        );
        sorted.dedup();
        assert_eq!(sorted.len(), TOOL_CATALOG.len(), "duplicate catalog entry");
    }

    #[test]
    fn both_spellings_of_a_tool_are_accepted() {
        assert!(is_known_tool("brain_search"));
        assert!(is_known_tool("mcp__harvey__brain_search"));
        assert!(is_known_tool("write_file"));
    }

    #[test]
    fn per_machine_pattern_tools_are_accepted_by_prefix() {
        // Not in the catalog and never will be — they are discovered at boot.
        assert!(!TOOL_CATALOG.contains(&"pattern_summarize"));
        assert!(is_known_tool("pattern_summarize"));
        assert!(is_known_tool("mcp__harvey__pattern_anything_at_all"));
    }

    #[test]
    fn invented_names_are_rejected() {
        assert!(
            !is_known_tool("web_search"),
            "never registered — see wizard note"
        );
        assert!(!is_known_tool("made_up_tool"));
        assert!(!is_known_tool(""));
        assert_eq!(
            unknown_tools(&["brain_search".to_string(), "made_up".to_string()]),
            vec!["made_up".to_string()]
        );
    }

    #[test]
    fn every_wizard_choice_is_a_real_tool() {
        // The wizard offering a tool that does not exist is exactly the bug
        // this module was written to close.
        for choice in WIZARD_TOOL_CHOICES {
            assert!(is_known_tool(choice), "wizard offers unknown tool {choice}");
        }
    }
}
