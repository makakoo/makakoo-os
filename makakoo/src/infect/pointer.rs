//! Pointer block — a tiny marker-bracketed redirect that tells the AI to
//! read the canonical bootstrap.
//!
//! v12 of the infect format (2026-04-25) moved away from copying the full
//! 200-line bootstrap into every CLI's global slot. Instead, every slot
//! now holds a ~15-line pointer that names the canonical file at
//! `$MAKAKOO_HOME/bootstrap/global.md` and tells the AI to read it.
//!
//! Why this is better than copies:
//!
//! - **Single source of truth.** Edit the canonical file once and every
//!   CLI sees the new content on its next session — no `makakoo infect`
//!   rerun required for content changes.
//! - **Lower drift risk.** The 15-line pointer is identical across all 8
//!   CLIs, so verify-deep is a marker check, not a full content diff.
//! - **Universal.** Adding a new CLI only requires a new slot path; the
//!   pointer body is format-agnostic markdown that any LLM can follow.
//! - **Resilient.** The pointer carries a tiny "quick identity" fallback
//!   so the AI still knows it's Harvey even if the canonical file is
//!   unreachable for any reason.
//!
//! The marker brackets (`<!-- harvey:infect-global START v12 --> ... END`)
//! are still managed by [`super::writer::render_markdown_block`] and
//! [`super::writer::render_opencode_entry`]; this module renders only the
//! body that goes between them.

use std::path::Path;

/// Render the pointer body for the markdown slots. Caller wraps it with
/// the START/END markers via [`super::writer::render_markdown_block`].
pub fn render_pointer_body(canonical_path: &Path) -> String {
    let path_str = canonical_path.display();
    format!(
        "**IMPORTANT — Makakoo OS bootstrap.** You are running on a machine with Makakoo OS installed. Your full persona, tools, and operating rules live in this file:\n\
         \n\
         ```\n\
         {path_str}\n\
         ```\n\
         \n\
         **Read that file now** and follow every rule in it. This pointer is rewritten by `makakoo infect`; the canonical content can be updated without re-infecting because every CLI re-reads the path above each session.\n\
         \n\
         **Quick identity (fallback if the canonical file is unreachable):**\n\
         - You are **Harvey**, the user's autonomous cognitive extension. Zero sycophancy, sharp and concise, slightly blunt. You are the boss of all other agents.\n\
         - The platform is **Makakoo OS**; the persona is **Harvey** (renamed 2026-04-14 from Harvey OS — persona unchanged).\n\
         - Never auto-send emails / LinkedIn / Slack — draft first, get explicit approval.\n\
         - After significant work, append `- [[Topic]] summary` to `$MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md` and run `makakoo sync`.\n\
         - Your CLI surface: `makakoo {{infect, search, query, sancho, plugin, daemon, sync, secret, perms}}`. Run `makakoo --help` for the authoritative list."
    )
}

/// Render the pointer body for the OpenCode JSON slot. OpenCode stores
/// instructions as an array of strings; we collapse the markdown pointer
/// into a single string so it survives JSON encoding cleanly. The actual
/// `[harvey:infect-global v12]` tag prefix is added by
/// [`super::writer::render_opencode_entry`].
pub fn render_pointer_for_opencode(canonical_path: &Path) -> String {
    let path_str = canonical_path.display();
    format!(
        "Makakoo OS bootstrap pointer. Read {path_str} now and follow every rule in it. \
         You are Harvey — the user's autonomous extension. Zero sycophancy, sharp, slightly blunt. \
         Never auto-send messages. After significant work, append to today's Brain journal at \
         $MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md and run `makakoo sync`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pointer_body_includes_canonical_path() {
        let p = PathBuf::from("/Users/sebastian/MAKAKOO/bootstrap/global.md");
        let body = render_pointer_body(&p);
        assert!(body.contains("/Users/sebastian/MAKAKOO/bootstrap/global.md"));
        assert!(body.contains("**Harvey**"));
        assert!(body.contains("`makakoo sync`"));
    }

    #[test]
    fn pointer_body_is_short() {
        // The whole point of v12 — slot content should be ~15-25 lines, not 200+.
        let p = PathBuf::from("/x/y/z.md");
        let body = render_pointer_body(&p);
        let lines = body.lines().count();
        assert!(
            lines < 30,
            "pointer body should stay under 30 lines, got {lines}"
        );
    }

    #[test]
    fn opencode_pointer_is_single_line_friendly() {
        let p = PathBuf::from("/x/y/z.md");
        let body = render_pointer_for_opencode(&p);
        // OpenCode stores it as one JSON string — keep it compact and
        // free of literal newlines that would explode the JSON encoding.
        assert!(!body.contains('\n'));
        assert!(body.contains("/x/y/z.md"));
        assert!(body.contains("Harvey"));
    }
}
