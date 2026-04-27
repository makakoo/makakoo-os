//! Wiki compiler — normalises freeform markdown into a Logseq-ready
//! outliner page and (optionally) injects a property header.
//!
//! This is the Rust counterpart to the small "save_answer"-style path
//! in `core/superbrain/wiki.py`: we take a blob of markdown produced by
//! some other subsystem (an MCP tool response, a SANCHO synthesis, a
//! skill output) and turn it into bullets that survive a lint pass.
//!
//! Normalisation rules:
//!
//! * Existing bullet lines (`- foo`, `  - nested`) are preserved as-is.
//! * Markdown headers (`# Title`) become `- # Title` so they remain
//!   visible in Logseq's outliner view.
//! * Other non-empty lines become `- line`.
//! * Consecutive blank lines collapse to a single blank so Logseq
//!   doesn't pad pages with visual noise.
//! * Wikilinks are left untouched — they are already Logseq-native.
//! * Optional property header (from `CompileOptions::properties`) is
//!   prepended as a Logseq property block (`key:: value`) followed by
//!   an empty separator bullet.

use serde::{Deserialize, Serialize};

/// Optional knobs for a compilation run. Everything defaults to "leave
/// the content alone" so the common case stays a no-op.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompileOptions {
    /// Optional page title. If set, prepended as `- {title}`.
    pub title: Option<String>,
    /// Logseq property rows in the order they should appear.
    pub properties: Vec<(String, String)>,
    /// Collapse consecutive blank lines to at most one.
    pub collapse_blanks: bool,
}

/// Output of `WikiCompiler::compile`. Carries both the normalised
/// source and a count of the transformations that fired so callers can
/// surface "n lines rewritten" in a CLI or a test assertion.
#[derive(Clone, Debug)]
pub struct CompiledPage {
    pub content: String,
    pub lines_rewritten: usize,
    pub lines_total: usize,
}

#[derive(Clone, Debug, Default)]
pub struct WikiCompiler;

impl WikiCompiler {
    pub fn new() -> Self {
        Self
    }

    /// Compile a markdown blob into a Logseq-ready page.
    pub fn compile(&self, source: &str, opts: &CompileOptions) -> CompiledPage {
        let mut out: Vec<String> = Vec::new();
        let mut rewritten = 0usize;

        if let Some(title) = &opts.title {
            out.push(format!("- {title}"));
        }
        for (k, v) in &opts.properties {
            out.push(format!("  - {k}:: {v}"));
        }
        if opts.title.is_some() || !opts.properties.is_empty() {
            out.push("-".to_string());
        }

        let mut prev_blank = false;
        for raw in source.lines() {
            let trimmed = raw.trim_end();

            if trimmed.is_empty() {
                if opts.collapse_blanks && prev_blank {
                    continue;
                }
                out.push(String::new());
                prev_blank = true;
                continue;
            }
            prev_blank = false;

            let leading_ws: String = trimmed
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let body = trimmed.trim_start();

            // Already a bullet — preserve verbatim.
            if body.starts_with("- ") || body == "-" {
                out.push(trimmed.to_string());
                continue;
            }
            // Dash with no following space: rewrite `-foo` → `- foo`.
            if let Some(rest) = body.strip_prefix('-') {
                out.push(format!("{leading_ws}- {}", rest.trim_start()));
                rewritten += 1;
                continue;
            }
            // Header: `# Title` → `- # Title`.
            if body.starts_with('#') {
                out.push(format!("{leading_ws}- {body}"));
                rewritten += 1;
                continue;
            }
            // Property line: keep as nested bullet.
            if body.contains("::") && is_property_like(body) {
                out.push(format!("{leading_ws}- {body}"));
                rewritten += 1;
                continue;
            }
            // Plain prose — wrap in a bullet.
            out.push(format!("{leading_ws}- {body}"));
            rewritten += 1;
        }

        // Ensure trailing newline.
        let mut content = out.join("\n");
        if !content.ends_with('\n') {
            content.push('\n');
        }

        CompiledPage {
            content,
            lines_rewritten: rewritten,
            lines_total: out.len(),
        }
    }
}

fn is_property_like(body: &str) -> bool {
    if let Some(idx) = body.find("::") {
        let key = &body[..idx];
        !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiki::lint::WikiLinter;

    #[test]
    fn plain_prose_becomes_bullets_and_passes_lint() {
        let source = "Harvey is autonomous\nHe runs on switchAILocal\n";
        let compiled = WikiCompiler::new().compile(source, &CompileOptions::default());
        assert_eq!(compiled.lines_rewritten, 2);
        let report = WikiLinter::new().lint_str(&compiled.content, None);
        assert!(report.is_clean(), "{:?}", report.issues);
    }

    #[test]
    fn existing_bullets_are_preserved() {
        let source = "- [[Harvey]]\n  - already nested\n- plain\n";
        let compiled = WikiCompiler::new().compile(source, &CompileOptions::default());
        assert_eq!(compiled.lines_rewritten, 0);
        assert!(compiled.content.contains("- [[Harvey]]"));
    }

    #[test]
    fn title_and_properties_render_as_header_block() {
        let opts = CompileOptions {
            title: Some("Harvey Log".to_string()),
            properties: vec![
                ("type".to_string(), "synthesis".to_string()),
                ("generated".to_string(), "2026-04-14".to_string()),
            ],
            collapse_blanks: false,
        };
        let compiled = WikiCompiler::new().compile("- first body line\n", &opts);
        assert!(compiled.content.starts_with("- Harvey Log\n"));
        assert!(compiled.content.contains("  - type:: synthesis"));
        assert!(compiled.content.contains("  - generated:: 2026-04-14"));
    }

    #[test]
    fn collapse_blanks_removes_consecutive_empties() {
        let source = "- a\n\n\n\n- b\n";
        let opts = CompileOptions {
            collapse_blanks: true,
            ..Default::default()
        };
        let compiled = WikiCompiler::new().compile(source, &opts);
        let blank_count = compiled.content.matches("\n\n").count();
        assert_eq!(blank_count, 1);
    }
}
