//! CLI slot definitions — the 7 global config files that the infect system
//! writes the Makakoo bootstrap block into.
//!
//! Paths mirror `core/orchestration/infect_global.py` exactly so the Rust
//! rewrite and the Python implementation can operate on the same files
//! without fighting over format. Bootstrap version is currently **v12**;
//! the regex `BLOCK_RE` still matches any prior version so the rewrite
//! will upgrade older blocks in place on the next run.

use std::path::PathBuf;

/// Marker version used by the Rust rewrite. Every material change to the
/// bootstrap body bumps this — anything older is considered stale and will
/// be replaced in place on next `makakoo infect`.
///
/// v12 (2026-04-25): pointer pattern. Each slot now holds a ~15-line
///   pointer to `$MAKAKOO_HOME/bootstrap/global.md` instead of a 200-line
///   copy of the bootstrap. Edit the canonical file once, every CLI sees
///   the new content next session — no re-infect needed for content edits.
///   Codex slot moved from `.codex/instructions.md` to `AGENTS.md` so it
///   actually gets read by modern Codex CLI (it walks up looking for
///   AGENTS.md and never reads .codex/instructions.md by default).
/// v11 (2026-04-20): harvey-os/ references stripped — bootstrap now
///   points at `harvey skill info`/`$MAKAKOO_HOME/config/persona.json`
///   and kernel commands instead of python module paths inside
///   `harvey-os/core/`. Phase 4 of SPRINT-KILL-HARVEYOS-PYTHONPATH.
/// v10 (2026-04-20): describe-vs-ingest dichotomy + rate-limit rule.
pub const BLOCK_VERSION: &str = "12";

/// Start marker written to every markdown slot. Keeps the legacy
/// `harvey:infect-global` prefix so old installations with v8 blocks are
/// matched by the upgrade regex and replaced — do NOT rename this to
/// `makakoo:infect-global` without also teaching [`crate::infect::writer`]
/// to find both.
pub const BLOCK_START: &str = "<!-- harvey:infect-global START v12 -->";

/// End marker — the version is NOT included in the end marker (mirrors
/// Python) so the upgrade regex can match any prior version cleanly.
pub const BLOCK_END: &str = "<!-- harvey:infect-global END -->";

/// JSON-tag prefix used by the OpenCode slot, which stores the bootstrap as
/// an entry inside `instructions: [...]` rather than as a fenced block.
pub const JSON_TAG_PREFIX: &str = "[harvey:infect-global v12]";

/// Fingerprint checked against the first 40 characters of each entry in
/// the opencode `instructions` array to locate the prior bootstrap.
pub const JSON_TAG_FINGERPRINT: &str = "[harvey:infect-global";

/// How a slot serializes the bootstrap on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotFormat {
    /// Fenced markdown block with `<!-- harvey:infect-global START/END -->`
    /// markers. Most slots use this.
    Markdown,
    /// JSON file with a top-level `instructions: []` array; the bootstrap
    /// is stored as a tagged string entry in that array.
    OpencodeJson,
}

/// One CLI global-config slot. Paths are relative to `$HOME` — use
/// [`crate::infect::writer::expand_tilde`] to get an absolute path.
#[derive(Debug, Clone)]
pub struct CliSlot {
    /// Short human name — `claude`, `gemini`, `codex`, …
    pub name: &'static str,
    /// Path template relative to `$HOME`, POSIX-style.
    pub rel_path: &'static str,
    /// Serialization format for this slot.
    pub format: SlotFormat,
}

impl CliSlot {
    /// Absolute path for this slot under the given `$HOME`.
    pub fn absolute(&self, home: &std::path::Path) -> PathBuf {
        home.join(self.rel_path)
    }
}

/// Canonical 8-slot list (v0.2 B.1: pi added 2026-04-21). Verified against
/// `plugins-core/lib-harvey-core/src/core/orchestration/infect_global.py::SLOTS`.
pub const SLOTS: &[CliSlot] = &[
    CliSlot {
        name: "claude",
        rel_path: ".claude/CLAUDE.md",
        format: SlotFormat::Markdown,
    },
    CliSlot {
        name: "gemini",
        rel_path: ".gemini/GEMINI.md",
        format: SlotFormat::Markdown,
    },
    // Codex: modern Codex CLI walks up from cwd looking for AGENTS.md
    // files at "/", in "~", and in git repos (its own docs verbatim). It
    // does NOT read `.codex/instructions.md` unless `model_instructions_file`
    // is explicitly set in `~/.codex/config.toml` — which we do not require
    // users to configure. So the codex slot is the home-level `~/AGENTS.md`.
    // The infect writer upserts a marker-bracketed block, so any pre-existing
    // ~/AGENTS.md content (e.g. tytus instructions) is preserved alongside.
    // Bug fixed 2026-04-25 — codex previously wrote to .codex/instructions.md
    // and Codex never picked it up: identity stayed "Codex", not "Harvey".
    CliSlot {
        name: "codex",
        rel_path: "AGENTS.md",
        format: SlotFormat::Markdown,
    },
    CliSlot {
        name: "opencode",
        rel_path: ".config/opencode/opencode.json",
        format: SlotFormat::OpencodeJson,
    },
    CliSlot {
        name: "vibe",
        rel_path: ".vibe/instructions.md",
        format: SlotFormat::Markdown,
    },
    CliSlot {
        name: "cursor",
        rel_path: ".cursor/rules.md",
        format: SlotFormat::Markdown,
    },
    CliSlot {
        name: "qwen",
        rel_path: ".qwen/QWEN.md",
        format: SlotFormat::Markdown,
    },
    // pi (badlogic/pi-mono) — 8th host, added in v0.2 Phase B.1.
    // Bootstrap lives at `.pi/AGENTS.md`, memory is symlinked to the
    // shared auto-memory dir alongside every other slot via
    // `infect::memory::sync_slot_memory_symlink`.
    CliSlot {
        name: "pi",
        rel_path: ".pi/AGENTS.md",
        format: SlotFormat::Markdown,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_slots_match_python() {
        assert_eq!(SLOTS.len(), 8);
        let names: Vec<&str> = SLOTS.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "claude", "gemini", "codex", "opencode", "vibe", "cursor",
                "qwen", "pi"
            ]
        );
    }

    #[test]
    fn pi_slot_targets_agents_md() {
        let pi = SLOTS
            .iter()
            .find(|s| s.name == "pi")
            .expect("pi slot must exist in the 8-slot list");
        assert_eq!(pi.rel_path, ".pi/AGENTS.md");
        assert!(matches!(pi.format, SlotFormat::Markdown));
    }

    #[test]
    fn opencode_is_json_others_are_markdown() {
        for slot in SLOTS {
            if slot.name == "opencode" {
                assert_eq!(slot.format, SlotFormat::OpencodeJson);
            } else {
                assert_eq!(slot.format, SlotFormat::Markdown);
            }
        }
    }

    #[test]
    fn markers_versioned_to_v12() {
        assert_eq!(BLOCK_VERSION, "12");
        assert!(BLOCK_START.contains("v12"));
        assert!(JSON_TAG_PREFIX.contains("v12"));
        // END marker intentionally has no version — it matches any prior version.
        assert!(!BLOCK_END.contains("v12"));
        assert!(!BLOCK_END.contains("v11"));
    }

    #[test]
    fn codex_slot_targets_home_agents_md() {
        // 2026-04-25 regression — codex previously wrote to
        // `.codex/instructions.md` which modern Codex CLI does not read.
        // The slot must point at `AGENTS.md` at HOME, which Codex walks
        // up from cwd to find natively.
        let codex = SLOTS
            .iter()
            .find(|s| s.name == "codex")
            .expect("codex slot must exist");
        assert_eq!(codex.rel_path, "AGENTS.md");
        assert!(matches!(codex.format, SlotFormat::Markdown));
    }
}
