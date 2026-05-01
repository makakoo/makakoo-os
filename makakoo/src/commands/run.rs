//! `makakoo run pattern=NAME` — pattern dispatch CLI verb.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1 Phase 2.
//!
//! Workflow:
//!   1. Look up pattern in PluginRegistry by name (with or without
//!      `pattern-` prefix).
//!   2. Read sibling `system.md`.
//!   3. Resolve strategy + mascot + model + vendor (Phase 3 resolver).
//!   4. Load strategy + mascot bodies if applicable.
//!   5. Build the provided-variables map from `--var` + `--input`.
//!   6. Compose using the shared makakoo-core composer (reused by
//!      Phase 5 MCP auto-expose — do not copy-paste).
//!   7. If `--dry-run`, print and exit. Else fire switchAILocal via
//!      makakoo-core::llm::LlmClient.
//!   8. Emit response on stdout. With `--json`, validate parseable
//!      JSON; non-JSON exits 2 with the body on stderr.

use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{anyhow, bail, Context};

use makakoo_core::llm::{ChatMessage, LlmClient};
use makakoo_core::platform::makakoo_home;
use makakoo_core::plugin::{PluginKind, PluginRegistry};
use makakoo_core::run::{
    compose, load_mascot, load_strategy, resolve_route, ComposeRequest,
};

use crate::context::CliContext;

/// Run a pattern.
///
/// `home_override` is None in production; tests pass a TempDir so the
/// registry walk operates on a controlled plugins/ tree.
pub async fn run(
    pattern_name: &str,
    input: Option<String>,
    vars: Vec<String>,
    mascot_override: Option<String>,
    strategy_override: Option<String>,
    model_override: Option<String>,
    vendor_override: Option<String>,
    dry_run: bool,
    json: bool,
    _ctx: &CliContext,
) -> anyhow::Result<i32> {
    let canonical = canonical_pattern_dirname(pattern_name);
    let registry = PluginRegistry::load_default(&makakoo_home())
        .with_context(|| format!("loading plugin registry from {}", makakoo_home().display()))?;
    let plugin = registry
        .get(&canonical)
        .ok_or_else(|| anyhow!("pattern {pattern_name:?} not found in registry (looked up {canonical:?})"))?;
    if plugin.manifest.plugin.kind != PluginKind::Pattern {
        bail!(
            "{canonical} exists but is kind={:?}, not pattern",
            plugin.manifest.plugin.kind
        );
    }
    let pattern_table = plugin
        .manifest
        .pattern
        .as_ref()
        .ok_or_else(|| anyhow!("{canonical} has kind=pattern but no [pattern] table"))?;

    // Read sibling system.md.
    let system_md_path = plugin.root.join("system.md");
    let system_md = std::fs::read_to_string(&system_md_path).with_context(|| {
        format!("reading {} for pattern {canonical}", system_md_path.display())
    })?;

    // Resolve route (Phase 3).
    let route = resolve_route(
        &plugin.manifest.plugin.name,
        pattern_table,
        model_override.as_deref(),
        vendor_override.as_deref(),
        |k| std::env::var(k).ok(),
    );

    // Resolve strategy choice.
    let strategy_name = strategy_override
        .as_deref()
        .or(pattern_table.strategy_default.as_deref());
    let strategy_text = match strategy_name {
        Some(name) if name != "none" => Some(
            load_strategy(name, None)
                .with_context(|| format!("loading strategy {name:?}"))?,
        ),
        _ => None,
    };

    // Resolve mascot choice.
    let mascot_name = mascot_override
        .as_deref()
        .or(pattern_table.mascot_default.as_deref());
    let mascot_text = match mascot_name {
        Some(name) if name != "none" => Some(
            load_mascot(name, None)
                .with_context(|| format!("loading mascot persona {name:?}"))?,
        ),
        _ => None,
    };

    // Build provided-variables map.
    let mut provided: BTreeMap<String, String> = BTreeMap::new();
    if let Some(raw) = &input {
        let resolved = resolve_input(raw)?;
        provided.insert("input".to_string(), resolved);
    }
    for kv in &vars {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| anyhow!("--var must be NAME=VALUE, got {kv:?}"))?;
        provided.insert(k.to_string(), v.to_string());
    }

    // Compose.
    let composed = compose(&ComposeRequest {
        pattern: pattern_table,
        system_md: &system_md,
        strategy_text: strategy_text.as_deref(),
        mascot_text: mascot_text.as_deref(),
        provided_vars: &provided,
    })
    .with_context(|| format!("composing pattern {canonical}"))?;

    if dry_run {
        println!("# pattern: {}", plugin.manifest.plugin.name);
        println!("# model: {}", route.model);
        println!("# vendor: {}", route.vendor);
        if let Some(s) = strategy_name {
            println!("# strategy: {s}");
        }
        if let Some(m) = mascot_name {
            println!("# mascot: {m}");
        }
        println!();
        println!("---- system ----");
        println!("{}", composed.system);
        println!("---- user ----");
        println!("{}", composed.user);
        return Ok(0);
    }

    // Build chat messages and dispatch.
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(2);
    if !composed.system.is_empty() {
        messages.push(ChatMessage::system(&composed.system));
    }
    if !composed.user.is_empty() {
        messages.push(ChatMessage::user(&composed.user));
    } else if messages.is_empty() {
        // Edge case: pattern body was empty AND no user input. The LLM
        // call would fail with no messages; bail with a clear error.
        bail!("pattern {canonical} has empty system + empty user body — nothing to send");
    }

    let client = LlmClient::new();
    let response = client
        .chat(&route.model, messages)
        .await
        .with_context(|| format!("dispatching pattern {canonical} to {}", route.model))?;

    if json {
        if let Err(parse_err) = serde_json::from_str::<serde_json::Value>(&response) {
            eprintln!("response was not valid JSON ({parse_err}):\n{response}");
            return Ok(2);
        }
    }

    println!("{response}");
    Ok(0)
}

/// Add the `pattern-` prefix if missing — pattern dirs use it
/// consistently in `plugins-core/`.
fn canonical_pattern_dirname(name: &str) -> String {
    if name.starts_with("pattern-") {
        name.to_string()
    } else {
        format!("pattern-{name}")
    }
}

/// Resolve an `--input` argument into a concrete string:
///   - `@/abs/path` or `@./rel/path` → file contents
///   - `-` → stdin (read to EOF)
///   - anything else → literal value
fn resolve_input(raw: &str) -> anyhow::Result<String> {
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading --input from stdin")?;
        return Ok(buf);
    }
    if let Some(path) = raw.strip_prefix('@') {
        return std::fs::read_to_string(path)
            .with_context(|| format!("reading --input from file {path:?}"));
    }
    Ok(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_dirname_adds_prefix() {
        assert_eq!(canonical_pattern_dirname("summarize"), "pattern-summarize");
        assert_eq!(
            canonical_pattern_dirname("pattern-summarize"),
            "pattern-summarize"
        );
    }

    #[test]
    fn resolve_input_literal() {
        assert_eq!(resolve_input("hello").unwrap(), "hello");
    }

    #[test]
    fn resolve_input_at_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "from-file").unwrap();
        let arg = format!("@{}", tmp.path().display());
        assert_eq!(resolve_input(&arg).unwrap(), "from-file");
    }

    #[test]
    fn resolve_input_at_missing_file_errors() {
        let err = resolve_input("@/definitely/does/not/exist/12345").unwrap_err();
        assert!(format!("{err}").contains("--input from file"));
    }
}
