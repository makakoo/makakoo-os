//! Pattern auto-expose: every `kind = "pattern"` plugin becomes
//! `pattern_<name>` MCP tool at boot.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1 Phase 5. The parasite payoff: write a
//! markdown file under `plugins-core/pattern-<name>/`, and on next
//! `makakoo-mcp` restart every infected CLI gains a native tool.
//!
//! ## P5.9 — Caveman default + tag bypass
//!
//! When invoked via MCP (computer-to-computer), patterns default to
//! the `caveman` strategy IF:
//!
//!   1. The pattern's `[pattern].strategy_default` is unset, AND
//!   2. The pattern's `[pattern].tags` does NOT include `external`
//!      or `polished` (those opt out — see Locked Decision 11), AND
//!   3. The MCP `_strategy` argument is not set, AND
//!   4. The MCP `_strategy` argument is not the literal `"none"`.
//!
//! CLI invocations (`makakoo run`) keep their existing neutral
//! default — the host CLI already governs voice there.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use makakoo_core::llm::{ChatMessage, LlmClient};
use makakoo_core::plugin::{Manifest, PatternTable, PluginKind, PluginRegistry, VariableKind};
use makakoo_core::run::{
    compose, load_mascot, load_strategy, resolve_route, ComposeRequest,
};

use crate::dispatch::{ToolContext, ToolHandler, ToolRegistry};
use crate::jsonrpc::RpcError;

/// One MCP tool per `kind = "pattern"` plugin discovered at boot.
pub struct PatternToolHandler {
    /// Pattern manifest, cloned at registration time. Stable for the
    /// server's lifetime — a pattern edit requires `makakoo-mcp` restart.
    manifest: Manifest,
    /// Plugin root directory — needed to read the sibling `system.md`.
    root: PathBuf,
    /// Pre-rendered `pattern_<name>` tool name.
    tool_name: String,
    /// Pre-rendered description seeded from `[pattern].description`
    /// plus a one-liner about caveman default and bypass.
    description: String,
    /// Pre-rendered JSON Schema derived from the pattern's variables.
    input_schema: Value,
    /// LLM client for dispatch.
    llm: Arc<LlmClient>,
}

impl PatternToolHandler {
    fn pattern_table(&self) -> &PatternTable {
        self.manifest
            .pattern
            .as_ref()
            .expect("pattern handler instantiated without [pattern] table")
    }
}

#[async_trait]
impl ToolHandler for PatternToolHandler {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let pattern = self.pattern_table();

        // Pull variables from params. Variables are top-level keys;
        // routing controls (`_strategy`, `_mascot`, `_model`,
        // `_vendor`, `_json`) start with underscore.
        let obj = params.as_object().ok_or_else(|| {
            RpcError::invalid_params("expected JSON object for pattern arguments")
        })?;

        let mut provided: BTreeMap<String, String> = BTreeMap::new();
        for v in &pattern.variables {
            if let Some(val) = obj.get(&v.name) {
                let s = match (val, v.kind) {
                    (Value::String(s), _) => s.clone(),
                    (other, VariableKind::Json) => other.to_string(),
                    (other, _) => other
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| other.to_string()),
                };
                provided.insert(v.name.clone(), s);
            }
        }

        let strategy_override = obj.get("_strategy").and_then(Value::as_str).map(str::to_string);
        let mascot_override = obj.get("_mascot").and_then(Value::as_str).map(str::to_string);
        let model_override = obj.get("_model").and_then(Value::as_str).map(str::to_string);
        let vendor_override = obj.get("_vendor").and_then(Value::as_str).map(str::to_string);
        let json_mode = obj.get("_json").and_then(Value::as_bool).unwrap_or(false);

        // P5.9 — caveman default for MCP-invoked patterns with tag
        // bypass. The decision is total: explicit `_strategy` always
        // wins, then pattern's strategy_default, then caveman if
        // tags don't opt out, then no strategy.
        let resolved_strategy = resolve_mcp_strategy(
            strategy_override.as_deref(),
            pattern.strategy_default.as_deref(),
            pattern,
        );

        let strategy_text = match resolved_strategy {
            Some(name) if name != "none" => Some(
                load_strategy(name, None)
                    .map_err(|e| RpcError::internal(format!("strategy {name:?}: {e}")))?,
            ),
            _ => None,
        };

        let mascot_name = mascot_override
            .as_deref()
            .or(pattern.mascot_default.as_deref());
        let mascot_text = match mascot_name {
            Some(name) if name != "none" => Some(
                load_mascot(name, None)
                    .map_err(|e| RpcError::internal(format!("mascot {name:?}: {e}")))?,
            ),
            _ => None,
        };

        let route = resolve_route(
            &self.manifest.plugin.name,
            pattern,
            model_override.as_deref(),
            vendor_override.as_deref(),
            |k| std::env::var(k).ok(),
        );

        // Read system.md.
        let system_md_path = self.root.join("system.md");
        let system_md = std::fs::read_to_string(&system_md_path).map_err(|e| {
            RpcError::internal(format!(
                "reading {} for pattern {}: {e}",
                system_md_path.display(),
                self.manifest.plugin.name
            ))
        })?;

        let composed = compose(&ComposeRequest {
            pattern,
            system_md: &system_md,
            strategy_text: strategy_text.as_deref(),
            mascot_text: mascot_text.as_deref(),
            provided_vars: &provided,
        })
        .map_err(|e| RpcError::invalid_params(format!("compose error: {e}")))?;

        // Build chat messages.
        let mut messages: Vec<ChatMessage> = Vec::with_capacity(2);
        if !composed.system.is_empty() {
            messages.push(ChatMessage::system(&composed.system));
        }
        if !composed.user.is_empty() {
            messages.push(ChatMessage::user(&composed.user));
        } else if messages.is_empty() {
            return Err(RpcError::invalid_params(
                "pattern composed to empty system + empty user — nothing to send".to_string(),
            ));
        }

        let response = self
            .llm
            .chat(&route.model, messages)
            .await
            .map_err(|e| RpcError::internal(format!("dispatch failure: {e}")))?;

        if json_mode {
            match serde_json::from_str::<Value>(&response) {
                Ok(parsed) => Ok(json!({ "content": parsed, "model": route.model })),
                Err(_) => Err(RpcError::internal(format!(
                    "_json was set but response was not parseable JSON: {response}"
                ))),
            }
        } else {
            Ok(json!({ "content": response, "model": route.model }))
        }
    }
}

/// Resolve which strategy MCP-invoked patterns should use. P5.9.
///
/// Precedence (highest first):
///   1. explicit `_strategy` arg (including `"none"` to disable)
///   2. pattern's `strategy_default`
///   3. `"caveman"` IF the pattern's `tags` does NOT opt out
///      (`external` or `polished`)
///   4. no strategy
fn resolve_mcp_strategy<'a>(
    explicit: Option<&'a str>,
    pattern_default: Option<&'a str>,
    pattern: &'a PatternTable,
) -> Option<&'a str> {
    if let Some(s) = explicit {
        return Some(s);
    }
    if let Some(s) = pattern_default {
        return Some(s);
    }
    if pattern.opts_out_of_caveman() {
        return None;
    }
    Some("caveman")
}

/// Build the JSON Schema for a pattern's variables. Returned as the
/// `input_schema` of the registered MCP tool.
fn build_input_schema(pattern: &PatternTable) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<String> = Vec::new();

    for v in &pattern.variables {
        let kind_schema = match v.kind {
            VariableKind::String => json!({ "type": "string" }),
            VariableKind::File => json!({
                "type": "string",
                "description": "filesystem path; the file's contents are substituted"
            }),
            VariableKind::Json => json!({
                "description": "any JSON value; serialized form is substituted"
            }),
        };
        let mut prop = kind_schema;
        if let Some(desc) = &v.description {
            prop["description"] = Value::String(desc.clone());
        }
        properties.insert(v.name.clone(), prop);
        if v.required {
            required.push(v.name.clone());
        }
    }

    // Routing controls. Documented but not required.
    properties.insert(
        "_strategy".into(),
        json!({
            "type": "string",
            "description": "Override the strategy. Built-in: cot, tot, react, harvey-rigor, caveman. Pass \"none\" to disable. MCP default is `caveman` unless the pattern's tags include external/polished."
        }),
    );
    properties.insert(
        "_mascot".into(),
        json!({
            "type": "string",
            "description": "Override the mascot persona overlay. Pass \"none\" to disable."
        }),
    );
    properties.insert(
        "_model".into(),
        json!({
            "type": "string",
            "description": "Override the model. Resolution: arg > pattern.toml > FABRIC_MODEL_<NAME> env > kernel default."
        }),
    );
    properties.insert(
        "_vendor".into(),
        json!({
            "type": "string",
            "description": "Override the vendor. Same precedence as _model sans env."
        }),
    );
    properties.insert(
        "_json".into(),
        json!({
            "type": "boolean",
            "description": "If true, validate the response is JSON; the result's `content` field carries the parsed value."
        }),
    );

    let mut schema = json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        schema["required"] = Value::Array(required.into_iter().map(Value::String).collect());
    }
    schema
}

/// Convert a pattern plugin name (`pattern-extract-wisdom`) to a tool
/// name (`pattern_extract_wisdom`). Hyphens become underscores so the
/// tool name matches the MCP naming convention `^[a-z][a-z0-9_]*$`.
fn pattern_tool_name(plugin_name: &str) -> String {
    plugin_name.replace('-', "_")
}

/// Build the auto-expose description.
fn pattern_tool_description(pattern: &PatternTable) -> String {
    let base = pattern
        .description
        .as_deref()
        .unwrap_or("Run this Makakoo pattern via switchAILocal — composed system prompt with strategy + mascot overlays.");
    let cav = if pattern.opts_out_of_caveman() {
        " Tagged external/polished — caveman default is skipped."
    } else {
        " MCP-invoked patterns default to the `caveman` strategy unless overridden via `_strategy`."
    };
    format!("{base}{cav}")
}

/// Walk the plugin registry under `ctx.home` and register one tool
/// per `kind = "pattern"` plugin. Patterns missing `system.md` were
/// already filtered out by the registry loader (Phase 1.4).
pub fn register_pattern_tools(registry: &mut ToolRegistry, ctx: Arc<ToolContext>) {
    let plugin_reg = match PluginRegistry::load_default(&ctx.home) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "pattern auto-expose: plugin registry load failed");
            return;
        }
    };

    let llm = ctx
        .llm
        .clone()
        .unwrap_or_else(|| Arc::new(LlmClient::default()));

    let mut count = 0usize;
    for plugin in plugin_reg.plugins() {
        if plugin.manifest.plugin.kind != PluginKind::Pattern {
            continue;
        }
        if !plugin.enabled {
            continue;
        }
        let Some(pat) = plugin.manifest.pattern.as_ref() else {
            continue;
        };
        let handler = PatternToolHandler {
            tool_name: pattern_tool_name(&plugin.manifest.plugin.name),
            description: pattern_tool_description(pat),
            input_schema: build_input_schema(pat),
            manifest: plugin.manifest.clone(),
            root: plugin.root.clone(),
            llm: Arc::clone(&llm),
        };
        registry.register(Arc::new(handler));
        count += 1;
    }
    tracing::info!(count, "pattern auto-expose: registered N pattern tools");
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::plugin::{PatternTable, VariableDecl, VariableKind};

    fn pat() -> PatternTable {
        PatternTable {
            description: Some("test pattern".into()),
            model: Some("gemini-2.5-flash-lite".into()),
            variables: vec![VariableDecl {
                name: "input".into(),
                description: Some("the input".into()),
                kind: VariableKind::String,
                required: true,
                default: None,
            }],
            ..Default::default()
        }
    }

    fn pat_external() -> PatternTable {
        PatternTable {
            tags: vec!["external".into()],
            ..pat()
        }
    }

    #[test]
    fn tool_name_replaces_hyphens() {
        assert_eq!(pattern_tool_name("pattern-summarize"), "pattern_summarize");
        assert_eq!(
            pattern_tool_name("pattern-extract-wisdom"),
            "pattern_extract_wisdom"
        );
    }

    #[test]
    fn input_schema_includes_required_variable() {
        let schema = build_input_schema(&pat());
        assert_eq!(schema["type"], "object");
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("input"));
        assert_eq!(props["input"]["type"], "string");
        let req = schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "input"));
    }

    #[test]
    fn input_schema_documents_routing_controls() {
        let schema = build_input_schema(&pat());
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("_strategy"));
        assert!(props.contains_key("_mascot"));
        assert!(props.contains_key("_model"));
        assert!(props.contains_key("_vendor"));
        assert!(props.contains_key("_json"));
    }

    #[test]
    fn description_mentions_caveman_default() {
        let desc = pattern_tool_description(&pat());
        assert!(desc.contains("caveman"));
    }

    #[test]
    fn description_for_external_tag_says_skipped() {
        let desc = pattern_tool_description(&pat_external());
        assert!(desc.contains("skipped"));
    }

    // ─── P5.9 caveman strategy resolution ────────────────────────────

    #[test]
    fn p59_no_explicit_no_default_no_tag_uses_caveman() {
        let pat = pat();
        let s = resolve_mcp_strategy(None, None, &pat);
        assert_eq!(s, Some("caveman"));
    }

    #[test]
    fn p59_external_tag_skips_caveman() {
        let pat = pat_external();
        let s = resolve_mcp_strategy(None, None, &pat);
        assert_eq!(s, None);
    }

    #[test]
    fn p59_polished_tag_skips_caveman() {
        let mut pat = pat();
        pat.tags = vec!["polished".into()];
        let s = resolve_mcp_strategy(None, None, &pat);
        assert_eq!(s, None);
    }

    #[test]
    fn p59_pattern_default_wins_over_caveman() {
        let mut pat = pat();
        pat.strategy_default = Some("harvey-rigor".into());
        let s = resolve_mcp_strategy(None, Some("harvey-rigor"), &pat);
        assert_eq!(s, Some("harvey-rigor"));
    }

    #[test]
    fn p59_explicit_strategy_overrides_caveman_and_pattern_default() {
        let mut pat = pat();
        pat.strategy_default = Some("harvey-rigor".into());
        let s = resolve_mcp_strategy(Some("cot"), Some("harvey-rigor"), &pat);
        assert_eq!(s, Some("cot"));
    }

    #[test]
    fn p59_explicit_none_disables_strategy_entirely() {
        // Even with no tag opt-out and no pattern default, explicit
        // `_strategy: "none"` should disable the strategy.
        let pat = pat();
        let s = resolve_mcp_strategy(Some("none"), None, &pat);
        assert_eq!(s, Some("none")); // composer maps "none" → no overlay
    }

    #[test]
    fn p59_unrelated_tags_do_not_skip_caveman() {
        let mut pat = pat();
        pat.tags = vec!["audit".into(), "security".into()];
        let s = resolve_mcp_strategy(None, None, &pat);
        assert_eq!(s, Some("caveman"));
    }

    // ─── End-to-end: temp MAKAKOO_HOME seeded with patterns ─────────

    fn seed_pattern(home: &std::path::Path, name: &str, body: &str, extra_toml: &str) {
        let dir = home.join("plugins").join(format!("pattern-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let toml = format!(
            r#"[plugin]
name = "pattern-{name}"
version = "0.1.0"
kind = "pattern"
language = "shell"
authors = ["test"]
license = "MIT"

[source]
path = "plugins/pattern-{name}"

[pattern]
description = "test pattern {name}"
model = "gemini-2.5-flash-lite"
{extra_toml}

[[pattern.variables]]
name = "input"
kind = "string"
required = true
"#
        );
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
        std::fs::write(dir.join("system.md"), body).unwrap();
    }

    #[test]
    fn register_pattern_tools_walks_seeded_home_and_registers() {
        use crate::dispatch::{ToolContext, ToolRegistry};

        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        seed_pattern(&home, "summarize", "Sum: {{input}}", "");
        seed_pattern(&home, "draft-email", "Draft: {{input}}", r#"tags = ["external"]"#);

        let ctx = Arc::new(ToolContext::empty(home));
        let mut registry = ToolRegistry::new();
        register_pattern_tools(&mut registry, ctx);

        let names: Vec<String> = registry
            .list()
            .into_iter()
            .map(|d| d.name.to_string())
            .collect();
        assert!(
            names.contains(&"pattern_summarize".to_string()),
            "expected pattern_summarize in {names:?}"
        );
        assert!(
            names.contains(&"pattern_draft_email".to_string()),
            "expected pattern_draft_email in {names:?}"
        );
    }

    #[test]
    fn external_tagged_pattern_description_says_caveman_skipped() {
        use crate::dispatch::{ToolContext, ToolRegistry};

        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        seed_pattern(&home, "draft-email", "Draft: {{input}}", r#"tags = ["external"]"#);

        let ctx = Arc::new(ToolContext::empty(home));
        let mut registry = ToolRegistry::new();
        register_pattern_tools(&mut registry, ctx);

        let descriptors = registry.list();
        let tool = descriptors
            .iter()
            .find(|d| d.name == "pattern_draft_email")
            .expect("draft_email registered");
        assert!(
            tool.description.contains("skipped"),
            "expected 'skipped' in description, got: {}",
            tool.description
        );
    }

    #[test]
    fn empty_home_registers_zero_patterns_cleanly() {
        use crate::dispatch::{ToolContext, ToolRegistry};

        let tmp = tempfile::tempdir().unwrap();
        let ctx = Arc::new(ToolContext::empty(tmp.path().to_path_buf()));
        let mut registry = ToolRegistry::new();
        register_pattern_tools(&mut registry, ctx);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn input_schema_required_drops_when_no_required_variables() {
        let mut p = pat();
        p.variables[0].required = false;
        let schema = build_input_schema(&p);
        assert!(
            schema.get("required").is_none(),
            "expected no `required` array when no variables are required"
        );
    }

    #[test]
    fn json_variable_emits_no_type_constraint() {
        let mut p = pat();
        p.variables[0].kind = VariableKind::Json;
        let schema = build_input_schema(&p);
        let props = schema["properties"].as_object().unwrap();
        let input_schema = &props["input"];
        // Json kind: no fixed `type` so any JSON value validates.
        assert!(input_schema.get("type").is_none());
    }
}
