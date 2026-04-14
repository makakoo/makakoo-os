//! Wiki linter — enforces Logseq outliner conventions on Brain pages
//! and journals.
//!
//! Rules (each is a `LintRule` variant and fires its own `LintIssue`):
//!
//! 1. **BulletPrefix** — every content line must either start with
//!    `- ` / `-\t` (outliner bullet), be empty, or be a property line
//!    (`key:: value`) or a header (`#`). Indented continuation bullets
//!    (`  - `) count as bullets.
//! 2. **Wikilinks** — every `[[` must have a matching `]]` on the same
//!    line. Unbalanced brackets are reported with the offending column.
//! 3. **JournalDate** — if the file stem matches
//!    `YYYY[_-]MM[_-]DD` it must parse cleanly as a date. Pure page
//!    files (non-journal) are not affected.
//! 4. **EmptyBullet** — a bullet line with no text after the dash
//!    (`- \n` or `-   \n`) is flagged; the sentinel separator `-`
//!    without trailing space is allowed (Python `wiki.py` uses it as a
//!    visual divider).
//!
//! The linter never mutates files; callers decide what to do with a
//! `LintReport`. `WikiLinter::lint_file` loads a file from disk;
//! `WikiLinter::lint_str` runs the same rules on an in-memory buffer
//! (used heavily by the unit tests).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Catalogue of every rule the linter enforces. New rules append to
/// this enum — adding a variant is the only place rule identity lives,
/// which keeps the lint report easy to filter in higher layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LintRule {
    BulletPrefix,
    Wikilinks,
    JournalDate,
    EmptyBullet,
}

impl LintRule {
    pub fn as_str(&self) -> &'static str {
        match self {
            LintRule::BulletPrefix => "bullet-prefix",
            LintRule::Wikilinks => "wikilinks",
            LintRule::JournalDate => "journal-date",
            LintRule::EmptyBullet => "empty-bullet",
        }
    }
}

/// Single lint violation. `line` is 1-indexed to match editor UX;
/// `column` is 1-indexed and may be `None` for file-level rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LintIssue {
    pub file: Option<PathBuf>,
    pub line: usize,
    pub column: Option<usize>,
    pub rule: LintRule,
    pub message: String,
}

/// Aggregate result of a single lint run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LintReport {
    pub issues: Vec<LintIssue>,
}

impl LintReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn count(&self, rule: LintRule) -> usize {
        self.issues.iter().filter(|i| i.rule == rule).count()
    }
}

/// Stateless lint engine — cheap to construct, safe to reuse.
#[derive(Clone, Debug, Default)]
pub struct WikiLinter;

impl WikiLinter {
    pub fn new() -> Self {
        Self
    }

    /// Lint an in-memory buffer without touching the filesystem. The
    /// optional `file` path lets callers attribute issues in a
    /// multi-file run.
    pub fn lint_str(&self, content: &str, file: Option<&Path>) -> LintReport {
        let mut issues: Vec<LintIssue> = Vec::new();
        let file_owned: Option<PathBuf> = file.map(|p| p.to_path_buf());

        // File-level rule: journal filename date check.
        if let Some(path) = file {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if looks_like_journal_stem(stem) && !is_valid_journal_stem(stem) {
                    issues.push(LintIssue {
                        file: file_owned.clone(),
                        line: 0,
                        column: None,
                        rule: LintRule::JournalDate,
                        message: format!(
                            "journal filename '{stem}' does not parse as YYYY_MM_DD"
                        ),
                    });
                }
            }
        }

        for (idx, raw_line) in content.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed_right = raw_line.trim_end();

            // Skip entirely empty lines.
            if trimmed_right.is_empty() {
                continue;
            }

            let leading_ws_len = trimmed_right.len() - trimmed_right.trim_start().len();
            let body = trimmed_right.trim_start();
            // For empty-bullet detection we need to know whether the
            // raw line had trailing whitespace after the dash — i.e.
            // "- " or "-   " vs a bare "-" separator. `had_trailing_ws`
            // is true when the raw line is longer than its right-trim.
            let had_trailing_ws = raw_line.len() > trimmed_right.len();

            // Rule 2: wikilink balance on the raw line. Checked even
            // for property/header lines because `[[...]]` is valid
            // outliner syntax in any context.
            if let Some(col) = find_unbalanced_wikilink(trimmed_right) {
                issues.push(LintIssue {
                    file: file_owned.clone(),
                    line: line_no,
                    column: Some(col),
                    rule: LintRule::Wikilinks,
                    message: "unbalanced [[ ]] on this line".to_string(),
                });
            }

            // Rule 1 + 4: bullet prefix and empty bullet.
            if is_allowed_non_bullet(body) {
                continue;
            }
            if !body.starts_with('-') {
                issues.push(LintIssue {
                    file: file_owned.clone(),
                    line: line_no,
                    column: Some(leading_ws_len + 1),
                    rule: LintRule::BulletPrefix,
                    message: "line does not start with '- ' (Logseq outliner)".to_string(),
                });
                continue;
            }

            // body starts with '-' — determine whether it's a valid
            // bullet, a separator ('-' alone with no trailing
            // whitespace), or an empty bullet ('- ' followed by only
            // whitespace).
            let after_dash = &body[1..];
            if after_dash.is_empty() {
                if had_trailing_ws {
                    // "-   " with only trailing whitespace: empty bullet.
                    issues.push(LintIssue {
                        file: file_owned.clone(),
                        line: line_no,
                        column: Some(leading_ws_len + 2),
                        rule: LintRule::EmptyBullet,
                        message: "bullet has no content after '- '".to_string(),
                    });
                }
                // Bare '-' with no trailing space is the Python
                // separator — not a real bullet, but not an error.
                continue;
            }
            // A proper bullet must start with whitespace after the
            // dash, then have non-whitespace payload.
            let first_char = after_dash.chars().next().unwrap();
            if !first_char.is_whitespace() {
                // e.g. `-foo` — treat as missing space after dash.
                issues.push(LintIssue {
                    file: file_owned.clone(),
                    line: line_no,
                    column: Some(leading_ws_len + 2),
                    rule: LintRule::BulletPrefix,
                    message: "bullet must have a space after '-'".to_string(),
                });
                continue;
            }
            let payload = after_dash.trim();
            if payload.is_empty() {
                issues.push(LintIssue {
                    file: file_owned.clone(),
                    line: line_no,
                    column: Some(leading_ws_len + 3),
                    rule: LintRule::EmptyBullet,
                    message: "bullet has no content after '- '".to_string(),
                });
            }
        }

        LintReport { issues }
    }

    /// Lint a file on disk.
    pub fn lint_file(&self, path: &Path) -> Result<LintReport> {
        let content = std::fs::read_to_string(path)?;
        Ok(self.lint_str(&content, Some(path)))
    }
}

// ─── helpers ────────────────────────────────────────────────────────

/// Lines that are accepted without a leading `-`: markdown headers
/// (`#`), Logseq page properties (`key:: value`), and fenced code
/// block delimiters (` ``` `).
fn is_allowed_non_bullet(body: &str) -> bool {
    body.starts_with('#')
        || body.starts_with("```")
        || is_property_line(body)
}

fn is_property_line(body: &str) -> bool {
    // `key:: value` — the key must be non-empty, may contain
    // alphanumerics / dashes / underscores, and must precede `::`.
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

/// Find the column of the first unbalanced `[[` / `]]` on a line.
/// Returns 1-indexed column or `None` if balanced.
fn find_unbalanced_wikilink(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut depth: i32 = 0;
    let mut last_open: Option<usize> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            depth += 1;
            last_open = Some(i + 1);
            i += 2;
            continue;
        }
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            depth -= 1;
            if depth < 0 {
                return Some(i + 1);
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    if depth != 0 {
        return Some(last_open.unwrap_or(0) + 1);
    }
    None
}

fn looks_like_journal_stem(stem: &str) -> bool {
    // YYYY_MM_DD or YYYY-MM-DD — 10 chars, digits + separator.
    stem.len() == 10
        && stem
            .chars()
            .enumerate()
            .all(|(idx, c)| match idx {
                4 | 7 => c == '_' || c == '-',
                _ => c.is_ascii_digit(),
            })
}

fn is_valid_journal_stem(stem: &str) -> bool {
    if !looks_like_journal_stem(stem) {
        return false;
    }
    let year: i32 = stem[0..4].parse().unwrap_or(0);
    let month: u32 = stem[5..7].parse().unwrap_or(0);
    let day: u32 = stem[8..10].parse().unwrap_or(0);
    chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_outliner_page_has_no_issues() {
        let content = "- [[Harvey]]\n  - autonomous cognitive extension\n  - boss of all agents\n";
        let report = WikiLinter::new().lint_str(content, None);
        assert!(report.is_clean(), "unexpected issues: {:?}", report.issues);
    }

    #[test]
    fn line_without_bullet_is_flagged() {
        let content = "Not a bullet\n- ok\n";
        let report = WikiLinter::new().lint_str(content, None);
        assert_eq!(report.count(LintRule::BulletPrefix), 1);
        assert_eq!(report.issues[0].line, 1);
    }

    #[test]
    fn empty_bullet_is_flagged() {
        let content = "- real\n-   \n- also real\n";
        let report = WikiLinter::new().lint_str(content, None);
        assert_eq!(report.count(LintRule::EmptyBullet), 1);
        assert_eq!(report.issues[0].line, 2);
    }

    #[test]
    fn unbalanced_wikilink_is_flagged() {
        let content = "- [[Harvey missing close\n- [[Makakoo OS]] fine\n";
        let report = WikiLinter::new().lint_str(content, None);
        assert_eq!(report.count(LintRule::Wikilinks), 1);
        assert_eq!(report.issues[0].line, 1);
    }

    #[test]
    fn property_and_header_lines_are_allowed() {
        let content = "# Title\ntype:: page\n- body\n";
        let report = WikiLinter::new().lint_str(content, None);
        assert!(report.is_clean(), "{:?}", report.issues);
    }

    #[test]
    fn bad_journal_filename_is_flagged() {
        use std::path::PathBuf;
        let path = PathBuf::from("/tmp/brain/journals/2026_13_99.md");
        let content = "- entry\n";
        let report = WikiLinter::new().lint_str(content, Some(&path));
        assert_eq!(report.count(LintRule::JournalDate), 1);
    }

    #[test]
    fn good_journal_filename_passes() {
        use std::path::PathBuf;
        let path = PathBuf::from("/tmp/brain/journals/2026_04_14.md");
        let content = "- entry\n";
        let report = WikiLinter::new().lint_str(content, Some(&path));
        assert!(report.is_clean(), "{:?}", report.issues);
    }
}
