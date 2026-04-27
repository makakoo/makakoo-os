//! High-level memory harness that wraps `superbrain::memory_stack` with
//! the filesystem I/O needed for autonomous context injection.
//!
//! The low-level [`crate::superbrain::memory_stack::MemoryStack`] is a
//! pure assembler: caller supplies identity text + semantic hits and it
//! returns a budgeted string. This harness handles the surrounding work
//! that every `makakoo query` call needs:
//!
//!   * **L0 (identity)** — load `SOUL.md` from a three-level cascade and
//!     truncate to a per-intent budget.
//!   * **L1 (today)** — read `data/Brain/journals/YYYY_MM_DD.md` and the
//!     top-N entities from the `entity_graph` table.
//!   * **L2 (per-query)** — call `SuperbrainStore::search` and trim each
//!     snippet to a fixed length.
//!
//! Intent is detected from the raw query string. Code intent gets a lean
//! L0 (every token counts during deep technical work); creative intent
//! gets the full Core Tone block (prevents drift to generic AI tone).

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Local;

use crate::error::Result;
use crate::superbrain::store::SuperbrainStore;

/// Default budgets in characters (≈ 4 chars per token).
const L0_CODE_BUDGET: usize = 160;
const L0_CREATIVE_BUDGET: usize = 1000;
const L0_DEFAULT_BUDGET: usize = 480;
const L1_JOURNAL_BUDGET: usize = 1200;
const L1_ENTITIES_BUDGET: usize = 400;
const L2_SNIPPET_CHARS: usize = 70;
const L2_TOP_K: usize = 5;

const SOUL_CORE_TONE_HEADER: &str = "## Core Tone";
const SOUL_NEXT_SECTION: &str = "\n## ";

const FALLBACK_IDENTITY: &str =
    "You are Makakoo — the user's autonomous cognitive extension. \
     Skip preamble. Return solutions, not questions. \
     Use tools, read schemas, run bash. Sharp, concise, hyper-competent.";

/// Detected intent of a user query — controls L0 budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Code,
    Creative,
    Default,
}

const CODE_KEYWORDS: &[&str] = &[
    "refactor", "debug", "compile", "stacktrace", "import",
    "function", " class ", " def ", "lint", "typecheck",
    "merge conflict", "rebase", "commit ", "pytest", "cargo ",
    " npm ", " test ",
];

const CODE_FILE_EXTENSIONS: &[&str] = &[
    ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".java",
    ".c", ".cpp", ".h", ".rb", ".php", ".swift", ".kt", ".sh",
    ".sql", ".yaml", ".yml", ".toml", ".json",
];

const CREATIVE_KEYWORDS: &[&str] = &[
    "draft", "write a", "compose", "essay", "story", "poem",
    "tweet", "linkedin", "email", "reply to",
];

/// Heuristic intent detection from raw query text.
///
/// Returns `Code` for queries that mention code (extensions, keywords,
/// fences), `Creative` for drafting/writing requests, `Default` otherwise.
pub fn detect_intent(query: &str) -> Intent {
    let blob = query.to_ascii_lowercase();
    if blob.contains("```") {
        return Intent::Code;
    }
    for ext in CODE_FILE_EXTENSIONS {
        if blob.contains(ext) {
            return Intent::Code;
        }
    }
    for kw in CODE_KEYWORDS {
        if blob.contains(kw) {
            return Intent::Code;
        }
    }
    for kw in CREATIVE_KEYWORDS {
        if blob.contains(kw) {
            return Intent::Creative;
        }
    }
    Intent::Default
}

fn l0_budget(intent: Intent) -> usize {
    match intent {
        Intent::Code => L0_CODE_BUDGET,
        Intent::Creative => L0_CREATIVE_BUDGET,
        Intent::Default => L0_DEFAULT_BUDGET,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut cut = max_chars;
    while !text.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    text[..cut].trim_end().to_string()
}

/// Memory harness — bundles the home dir + store handle so callers can
/// fetch a complete context block with one method call.
pub struct MemoryHarness {
    home: PathBuf,
    store: Arc<SuperbrainStore>,
}

impl MemoryHarness {
    pub fn new(home: PathBuf, store: Arc<SuperbrainStore>) -> Self {
        Self { home, store }
    }

    /// L0 + L1 — context block for session-start injection. Cheap, no
    /// query text required.
    pub fn bootstrap(&self, intent: Intent) -> String {
        let mut sections = Vec::new();
        let l0 = self.build_l0(intent);
        if !l0.is_empty() {
            sections.push(format!("## Identity\n{l0}"));
        }
        let l1 = self.build_l1();
        if !l1.is_empty() {
            sections.push(format!("## Today\n{l1}"));
        }
        sections.join("\n\n")
    }

    /// L0 + L1 + L2 — context block for a specific query. Runs FTS5 to
    /// pull L2 snippets via the underlying store.
    pub fn for_query(&self, query: &str, intent: Intent) -> String {
        let mut block = self.bootstrap(intent);
        let l2 = self.build_l2(query);
        if !l2.is_empty() {
            if !block.is_empty() {
                block.push_str("\n\n");
            }
            block.push_str("## Relevant\n");
            block.push_str(&l2);
        }
        block
    }

    fn build_l0(&self, intent: Intent) -> String {
        let cascade = [
            self.home.join("SOUL.md"),
            self.home.join("harvey-os").join("SOUL.md"),
        ];
        for path in &cascade {
            if let Ok(text) = std::fs::read_to_string(path) {
                if intent == Intent::Creative {
                    if let Some(core) = extract_core_tone(&text) {
                        return truncate(&core, l0_budget(intent));
                    }
                }
                let lean = first_meaningful_line(&text);
                if !lean.is_empty() {
                    return truncate(&lean, l0_budget(intent));
                }
            }
        }
        truncate(FALLBACK_IDENTITY, l0_budget(intent))
    }

    fn build_l1(&self) -> String {
        let today = Local::now().format("%Y_%m_%d").to_string();
        let journal_path = self
            .home
            .join("data")
            .join("Brain")
            .join("journals")
            .join(format!("{today}.md"));

        let mut parts = Vec::new();
        if let Ok(content) = std::fs::read_to_string(&journal_path) {
            let compressed = compress_journal(&content, L1_JOURNAL_BUDGET);
            if !compressed.is_empty() {
                parts.push(format!("Today: {compressed}"));
            }
        }
        if let Ok(entities) = self.top_entities(8) {
            if !entities.is_empty() {
                let line = format!("Key entities: {}", entities.join(", "));
                parts.push(truncate(&line, L1_ENTITIES_BUDGET));
            }
        }
        parts.join("\n")
    }

    fn build_l2(&self, query: &str) -> String {
        if query.trim().is_empty() {
            return String::new();
        }
        let hits = match self.store.search(query, L2_TOP_K) {
            Ok(h) => h,
            Err(_) => return String::new(),
        };
        let mut lines = Vec::new();
        for h in hits {
            let snippet = truncate(&h.content.replace('\n', " "), L2_SNIPPET_CHARS);
            lines.push(format!("- [{}] {}", h.doc_id, snippet));
        }
        lines.join("\n")
    }

    /// Top-N entity names from the graph nodes table. Falls back to an
    /// empty list if the schema isn't present yet.
    fn top_entities(&self, n: usize) -> Result<Vec<String>> {
        let conn = self.store.conn_arc();
        let conn = conn.lock().expect("brain conn poisoned");
        let mut stmt = match conn.prepare(
            "SELECT id FROM brain_graph_nodes
             WHERE node_type != 'journal'
             ORDER BY degree DESC
             LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Ok(Vec::new()),
        };
        let rows = stmt
            .query_map([n as i64], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap_or_default();
        Ok(rows)
    }
}

fn extract_core_tone(text: &str) -> Option<String> {
    let header_idx = text.find(SOUL_CORE_TONE_HEADER)?;
    let body_start = text[header_idx..].find('\n')? + header_idx + 1;
    let rest = &text[body_start..];
    let end = rest.find(SOUL_NEXT_SECTION).unwrap_or(rest.len());
    let body = rest[..end].trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

fn first_meaningful_line(text: &str) -> String {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cleaned = line
            .trim_start_matches('*')
            .trim_start_matches('-')
            .trim_start_matches('>')
            .trim()
            .replace("**", "");
        if cleaned.len() > 30 {
            return cleaned;
        }
    }
    String::new()
}

fn compress_journal(content: &str, budget: usize) -> String {
    let meaningful: Vec<String> = content
        .lines()
        .filter_map(|raw| {
            if raw.starts_with("    ") || raw.starts_with("\t\t") {
                return None;
            }
            let line = raw.trim();
            if line.is_empty() || line == "-" || line.starts_with("collapsed::") {
                return None;
            }
            let stripped = line.strip_prefix("- ").unwrap_or(line);
            Some(stripped.to_string())
        })
        .collect();
    if meaningful.is_empty() {
        return String::new();
    }
    let mut taken: Vec<String> = Vec::new();
    let mut chars = 0usize;
    for entry in meaningful.iter().rev() {
        let cost = entry.len() + 3;
        if chars + cost > budget {
            break;
        }
        taken.insert(0, entry.clone());
        chars += cost;
    }
    taken.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use tempfile::tempdir;

    fn make_store() -> (tempfile::TempDir, Arc<SuperbrainStore>) {
        let dir = tempdir().unwrap();
        let db = dir.path().join("sb.db");
        let conn = open_db(&db).unwrap();
        run_migrations(&conn).unwrap();
        drop(conn);
        let store = SuperbrainStore::open(&db).unwrap();
        (dir, Arc::new(store))
    }

    #[test]
    fn detect_intent_handles_code_keywords_and_extensions() {
        assert_eq!(detect_intent("refactor the auth"), Intent::Code);
        assert_eq!(detect_intent("review main.rs please"), Intent::Code);
        assert_eq!(detect_intent("```rust\nfn main(){}```"), Intent::Code);
        assert_eq!(detect_intent("draft a tweet about lope"), Intent::Creative);
        assert_eq!(detect_intent("what is sprint 002"), Intent::Default);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "héllo wörld";
        let out = truncate(s, 6);
        assert!(out.len() <= 6);
        assert!(s.starts_with(&out));
    }

    #[test]
    fn extract_core_tone_pulls_section_body() {
        let soul = "# Title\n\n## Core Tone\nLine A\nLine B\n\n## Next\nignored";
        let body = extract_core_tone(soul).unwrap();
        assert!(body.contains("Line A"));
        assert!(body.contains("Line B"));
        assert!(!body.contains("ignored"));
    }

    #[test]
    fn extract_core_tone_returns_none_when_missing() {
        assert!(extract_core_tone("# only title\n\nbody").is_none());
    }

    #[test]
    fn build_l0_falls_back_to_compiled_string() {
        let dir = tempdir().unwrap();
        let (_d, store) = make_store();
        let h = MemoryHarness::new(dir.path().to_path_buf(), store);
        let l0 = h.build_l0(Intent::Code);
        assert!(l0.contains("Makakoo") || l0.contains("autonomous"));
        assert!(l0.len() <= L0_CODE_BUDGET);
    }

    #[test]
    fn build_l0_reads_soul_cascade_legacy() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("harvey-os")).unwrap();
        std::fs::write(
            dir.path().join("harvey-os").join("SOUL.md"),
            "# SOUL\n\n## Core Tone\nYou are Harvey, the autonomous cognitive extension of Sebastian, sharp and concise.\n\n## Next\nignored",
        )
        .unwrap();
        let (_d, store) = make_store();
        let h = MemoryHarness::new(dir.path().to_path_buf(), store);
        let l0 = h.build_l0(Intent::Creative);
        assert!(l0.contains("Harvey"));
    }

    #[test]
    fn compress_journal_truncates_to_budget() {
        let body = "- one\n- two\n- three\n  - sub-bullet\n- four\n- five";
        let out = compress_journal(body, 30);
        assert!(out.len() <= 30, "got {}: {:?}", out.len(), out);
        assert!(out.contains("five"));
    }

    #[test]
    fn build_l1_with_today_journal() {
        let dir = tempdir().unwrap();
        let today = Local::now().format("%Y_%m_%d").to_string();
        let journals = dir.path().join("data").join("Brain").join("journals");
        std::fs::create_dir_all(&journals).unwrap();
        std::fs::write(
            journals.join(format!("{today}.md")),
            "- worked on Sprint 006 phase 1\n- closed flaky test loop",
        )
        .unwrap();
        let (_d, store) = make_store();
        let h = MemoryHarness::new(dir.path().to_path_buf(), store);
        let l1 = h.build_l1();
        assert!(l1.contains("Sprint 006"));
    }

    #[test]
    fn for_query_emits_relevant_section_when_hits_present() {
        let dir = tempdir().unwrap();
        let (_d, store) = make_store();
        // Seed one doc the FTS5 index will surface.
        store
            .write_document(
                "Sprint 002",
                "Sprint 002 finished plugin runtime correctness.",
                "page",
                serde_json::json!([]),
            )
            .unwrap();
        let h = MemoryHarness::new(dir.path().to_path_buf(), store);
        let block = h.for_query("Sprint 002", Intent::Default);
        assert!(block.contains("Identity"));
        assert!(block.contains("Relevant"));
        assert!(block.contains("Sprint 002"));
    }

    #[test]
    fn for_query_omits_relevant_when_query_empty() {
        let dir = tempdir().unwrap();
        let (_d, store) = make_store();
        let h = MemoryHarness::new(dir.path().to_path_buf(), store);
        let block = h.for_query("", Intent::Default);
        assert!(!block.contains("Relevant"));
    }
}
