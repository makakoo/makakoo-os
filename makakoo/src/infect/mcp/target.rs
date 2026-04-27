//! Per-CLI MCP target metadata.

use std::path::{Path, PathBuf};

/// Format family of a target's MCP config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpFormat {
    /// JSON file with a top-level `mcpServers: {}` object. Used by
    /// Claude, Gemini, Qwen, Cursor.
    JsonMcpServers,
    /// JSON file with a top-level `mcp: {}` object (OpenCode's
    /// variation of the JSON-mcpServers schema).
    JsonOpencode,
    /// TOML file with a `[mcp_servers.<name>]` inline-table section.
    /// Used by Codex.
    TomlInlineTable,
    /// TOML file with `[[mcp_servers]]` array-of-tables and a
    /// `transport` discriminator field. Used by Vibe.
    TomlArrayOfTables,
}

/// Every CLI we know how to infect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTarget {
    Claude,
    Gemini,
    Codex,
    OpenCode,
    Vibe,
    Qwen,
    Cursor,
}

impl McpTarget {
    /// All targets in canonical order. Stable across runs so report
    /// output doesn't shuffle.
    pub fn all() -> &'static [McpTarget] {
        &[
            McpTarget::Claude,
            McpTarget::Gemini,
            McpTarget::Codex,
            McpTarget::OpenCode,
            McpTarget::Vibe,
            McpTarget::Qwen,
            McpTarget::Cursor,
        ]
    }

    /// Short lowercased identifier (used in CLI output and `--target`
    /// filter).
    pub fn short_name(&self) -> &'static str {
        match self {
            McpTarget::Claude => "claude",
            McpTarget::Gemini => "gemini",
            McpTarget::Codex => "codex",
            McpTarget::OpenCode => "opencode",
            McpTarget::Vibe => "vibe",
            McpTarget::Qwen => "qwen",
            McpTarget::Cursor => "cursor",
        }
    }

    pub fn format(&self) -> McpFormat {
        match self {
            McpTarget::Claude => McpFormat::JsonMcpServers,
            McpTarget::Gemini => McpFormat::JsonMcpServers,
            McpTarget::Codex => McpFormat::TomlInlineTable,
            McpTarget::OpenCode => McpFormat::JsonOpencode,
            McpTarget::Vibe => McpFormat::TomlArrayOfTables,
            McpTarget::Qwen => McpFormat::JsonMcpServers,
            McpTarget::Cursor => McpFormat::JsonMcpServers,
        }
    }

    /// Path to the CLI's MCP config file, relative to `$HOME`.
    pub fn rel_path(&self) -> &'static str {
        match self {
            McpTarget::Claude => ".claude.json",
            McpTarget::Gemini => ".gemini/settings.json",
            McpTarget::Codex => ".codex/config.toml",
            McpTarget::OpenCode => ".config/opencode/opencode.json",
            McpTarget::Vibe => ".vibe/config.toml",
            McpTarget::Qwen => ".qwen/settings.json",
            McpTarget::Cursor => ".cursor/mcp.json",
        }
    }

    pub fn config_path_for_home(&self, home: &Path) -> PathBuf {
        home.join(self.rel_path())
    }

    /// Path to the bootstrap-instructions slot for this CLI (the file
    /// `infect --global` writes the bootstrap markdown into). Used by
    /// drift detection to confirm the slot has the expected markers.
    pub fn bootstrap_rel_path(&self) -> &'static str {
        match self {
            McpTarget::Claude => ".claude/CLAUDE.md",
            McpTarget::Gemini => ".gemini/GEMINI.md",
            // Codex moved off `.codex/instructions.md` 2026-04-25 — modern
            // Codex CLI walks up from cwd to find AGENTS.md and never reads
            // .codex/instructions.md unless `model_instructions_file` is set
            // in config.toml. The bootstrap now lives in `~/AGENTS.md`.
            McpTarget::Codex => "AGENTS.md",
            McpTarget::OpenCode => ".config/opencode/opencode.json",
            McpTarget::Vibe => ".vibe/instructions.md",
            McpTarget::Qwen => ".qwen/QWEN.md",
            McpTarget::Cursor => ".cursor/rules.md",
        }
    }

    /// Path under `$HOME` where the CLI keeps a per-user memory dir
    /// (symlinked to the canonical `data/auto-memory`).
    pub fn memory_rel_path(&self) -> Option<&'static str> {
        match self {
            McpTarget::Claude => Some(".claude/projects/-Users-sebastian-MAKAKOO/memory"),
            McpTarget::Gemini => Some(".gemini/memory"),
            McpTarget::Codex => Some(".codex/memory"),
            McpTarget::OpenCode => Some(".config/opencode/memory"),
            McpTarget::Vibe => Some(".vibe/memory"),
            McpTarget::Qwen => Some(".qwen/memory"),
            McpTarget::Cursor => Some(".cursor/memory"),
        }
    }

    /// Path under `$HOME` where the CLI keeps a per-user skills dir.
    /// Returns `None` for CLIs that don't use a shared-skills symlink
    /// (Claude/Codex/Cursor own their own skill systems).
    pub fn skills_rel_path(&self) -> Option<&'static str> {
        match self {
            McpTarget::Gemini => Some(".gemini/skills"),
            McpTarget::OpenCode => Some(".config/opencode/skills"),
            McpTarget::Vibe => Some(".vibe/skills"),
            McpTarget::Qwen => Some(".qwen/skills"),
            _ => None,
        }
    }

    /// Parse a short name back into a target. Used by `--target` CLI flag.
    pub fn from_short_name(name: &str) -> Option<Self> {
        for t in Self::all() {
            if t.short_name() == name {
                return Some(*t);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_seven_targets_present() {
        assert_eq!(McpTarget::all().len(), 7);
    }

    #[test]
    fn short_names_unique() {
        let names: Vec<_> = McpTarget::all().iter().map(|t| t.short_name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }

    #[test]
    fn format_assignments_match_known_cli_schemas() {
        assert_eq!(McpTarget::Claude.format(), McpFormat::JsonMcpServers);
        assert_eq!(McpTarget::Gemini.format(), McpFormat::JsonMcpServers);
        assert_eq!(McpTarget::Qwen.format(), McpFormat::JsonMcpServers);
        assert_eq!(McpTarget::Cursor.format(), McpFormat::JsonMcpServers);
        assert_eq!(McpTarget::OpenCode.format(), McpFormat::JsonOpencode);
        assert_eq!(McpTarget::Codex.format(), McpFormat::TomlInlineTable);
        assert_eq!(McpTarget::Vibe.format(), McpFormat::TomlArrayOfTables);
    }

    #[test]
    fn from_short_name_round_trips() {
        for t in McpTarget::all() {
            assert_eq!(McpTarget::from_short_name(t.short_name()), Some(*t));
        }
        assert_eq!(McpTarget::from_short_name("nope"), None);
    }

    #[test]
    fn config_paths_under_home() {
        let home = std::path::Path::new("/h");
        for t in McpTarget::all() {
            let p = t.config_path_for_home(home);
            assert!(p.starts_with(home));
            assert!(p.file_name().is_some());
        }
    }

    #[test]
    fn skills_rel_path_only_for_shared_skill_clis() {
        assert!(McpTarget::Vibe.skills_rel_path().is_some());
        assert!(McpTarget::Gemini.skills_rel_path().is_some());
        assert!(McpTarget::OpenCode.skills_rel_path().is_some());
        assert!(McpTarget::Qwen.skills_rel_path().is_some());
        assert!(McpTarget::Claude.skills_rel_path().is_none());
        assert!(McpTarget::Codex.skills_rel_path().is_none());
        assert!(McpTarget::Cursor.skills_rel_path().is_none());
    }
}
