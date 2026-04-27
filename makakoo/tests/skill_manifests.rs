//! Contract tests for every portable `SKILL.md` shipped in v0.4+ and
//! v0.5. Each skill manifest is validated for:
//!
//!   1. Frontmatter delimiters (leading `---` + closing `---`).
//!   2. Required YAML fields: `name`, `version`, `description`,
//!      `allowed-tools`.
//!   3. Non-empty markdown body after the frontmatter.
//!   4. Every tool listed in `allowed-tools` is referenced at least
//!      once in the body (proves the body documents what it claims).
//!
//! Adding a new SKILL.md? Extend `SKILL_PATHS` + `EXPECTED_TOOLS`.
//!
//! NOTE: this is deliberately a string-parse test, not a full YAML
//! parser. The SKILL.md format is stable and simple enough that a
//! regex-level check catches every real regression (missing fields,
//! corrupted frontmatter, unclaimed tools). Pulling in `serde_yaml`
//! just for four files would be overkill.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Every portable SKILL.md shipped in the tree. Path is relative to
/// the workspace root. Order does not matter.
const SKILL_PATHS: &[&str] = &[
    "plugins-core/agent-browser-harness/SKILL.md",
    "plugins-core/agent-multimodal-knowledge/SKILL.md",
    "plugins-core/agent-pi/SKILL.md",
    "plugins-core/lib-harvey-core/skills/agents/SKILL.md",
    "plugins-core/lib-harvey-core/skills/wiki/SKILL.md",
    "plugins-core/lib-harvey-core/skills/skill_discover/SKILL.md",
];

/// Expected `allowed-tools` entries per skill — keyed by the same
/// relative path. Each tool name MUST also appear somewhere in the
/// SKILL.md body.
fn expected_tools() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut m: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    m.insert(
        "plugins-core/agent-browser-harness/SKILL.md",
        vec!["harvey_browse"],
    );
    m.insert(
        "plugins-core/agent-multimodal-knowledge/SKILL.md",
        vec![
            "harvey_describe_image",
            "harvey_describe_audio",
            "harvey_describe_video",
            "harvey_generate_image",
            "harvey_knowledge_ingest",
        ],
    );
    m.insert(
        "plugins-core/agent-pi/SKILL.md",
        vec![
            "pi_run",
            "pi_session_fork",
            "pi_session_label",
            "pi_session_export",
            "pi_set_model",
            "pi_steer",
        ],
    );
    m.insert(
        "plugins-core/lib-harvey-core/skills/agents/SKILL.md",
        vec![
            "agent_list",
            "agent_info",
            "agent_create",
            "agent_install",
            "agent_uninstall",
        ],
    );
    m.insert(
        "plugins-core/lib-harvey-core/skills/wiki/SKILL.md",
        vec!["wiki_compile", "wiki_lint", "wiki_save"],
    );
    m.insert(
        "plugins-core/lib-harvey-core/skills/skill_discover/SKILL.md",
        vec!["skill_discover"],
    );
    m
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("makakoo crate sits inside workspace root")
        .to_path_buf()
}

/// Split a SKILL.md file into (frontmatter_body, markdown_body).
/// Panics if the `---` delimiters are missing — that's the whole
/// point of the validation.
fn split_frontmatter(text: &str, path: &Path) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        !lines.is_empty() && lines[0].trim() == "---",
        "{path:?}: expected leading `---` frontmatter delimiter",
    );
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim() == "---")
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{path:?}: missing closing `---` delimiter"));
    let fm = lines[1..close].join("\n");
    let body = lines[close + 1..].join("\n");
    (fm, body)
}

/// Check that a flat YAML-ish frontmatter declares every key in
/// `required`. Panics with a helpful message otherwise.
fn assert_has_keys(path: &Path, fm: &str, required: &[&str]) {
    for key in required {
        let needle_line = format!("{key}:");
        let found = fm
            .lines()
            .any(|l| l.trim_start().starts_with(needle_line.as_str()));
        assert!(
            found,
            "{path:?}: frontmatter missing required key {key:?}. \
             Frontmatter was:\n{fm}",
        );
    }
}

/// Parse the `allowed-tools:` block. The format is:
///
///   allowed-tools:
///     - foo
///     - bar
///
/// Returns the list of tool names.
fn parse_allowed_tools(path: &Path, fm: &str) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in fm.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("allowed-tools:") {
            in_block = true;
            continue;
        }
        if in_block {
            // Block ends at first non-indented non-empty line that's
            // not a bullet.
            let starts_indented = trimmed.starts_with("  ");
            if !trimmed.is_empty() && !starts_indented {
                break;
            }
            if let Some(rest) = trimmed.trim_start().strip_prefix("- ") {
                tools.push(rest.trim().to_string());
            }
        }
    }
    assert!(
        !tools.is_empty(),
        "{path:?}: allowed-tools: block is empty or malformed. \
         Frontmatter was:\n{fm}",
    );
    tools
}

#[test]
fn every_skill_md_parses_and_documents_its_tools() {
    let root = workspace_root();
    let expected = expected_tools();
    let mut failures: Vec<String> = Vec::new();

    for rel in SKILL_PATHS {
        let path = root.join(rel);
        if !path.is_file() {
            failures.push(format!("{rel}: file missing"));
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{rel}: read error: {e}"));
                continue;
            }
        };
        let (fm, body) = split_frontmatter(&text, &path);
        assert_has_keys(
            &path,
            &fm,
            &["name", "version", "description", "allowed-tools"],
        );

        let parsed_tools = parse_allowed_tools(&path, &fm);
        let Some(expected_list) = expected.get(rel) else {
            failures.push(format!(
                "{rel}: not in expected_tools() — add an entry before this test can validate it"
            ));
            continue;
        };

        // Parsed tools must match exactly (same set, any order).
        let parsed_set: std::collections::BTreeSet<&str> =
            parsed_tools.iter().map(|s| s.as_str()).collect();
        let expected_set: std::collections::BTreeSet<&str> =
            expected_list.iter().copied().collect();
        if parsed_set != expected_set {
            failures.push(format!(
                "{rel}: allowed-tools mismatch.\n  parsed:   {parsed_set:?}\n  expected: {expected_set:?}",
            ));
        }

        // Body must mention every declared tool at least once. Shorthand
        // is accepted when the body uses a family-prefix pattern:
        // `harvey_describe_image / _audio / _video` documents three
        // tools without spelling each prefix twice. Rule: exact match
        // OR (family-prefix present + trailing suffix present).
        for tool in expected_list {
            if body.contains(tool) {
                continue;
            }
            let last_underscore = tool.rfind('_');
            let shorthand_ok = match last_underscore {
                Some(i) if i > 0 && i + 1 < tool.len() => {
                    let prefix = &tool[..i + 1]; // includes trailing '_'
                    let suffix_token = &tool[i..]; // leading '_'
                    body.contains(prefix) && body.contains(suffix_token)
                }
                _ => false,
            };
            if !shorthand_ok {
                failures.push(format!(
                    "{rel}: declared tool {tool:?} is not mentioned in the markdown body \
                     (tried exact match + `prefix_/_suffix` shorthand)"
                ));
            }
        }

        // Body must not be empty — a frontmatter-only SKILL.md is a bug.
        assert!(
            body.lines().any(|l| !l.trim().is_empty()),
            "{rel}: markdown body is empty after frontmatter"
        );
    }

    if !failures.is_empty() {
        panic!(
            "\n\n❌ SKILL.md contract failures:\n  - {}\n",
            failures.join("\n  - ")
        );
    }
}

#[test]
fn skill_paths_cover_every_expected_entry() {
    let declared: std::collections::BTreeSet<&str> = SKILL_PATHS.iter().copied().collect();
    let expected: std::collections::BTreeSet<&str> = expected_tools().keys().copied().collect();
    assert_eq!(
        declared, expected,
        "SKILL_PATHS drifted from expected_tools(). Keep them in sync."
    );
}
