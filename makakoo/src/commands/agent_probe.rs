//! `makakoo agent health --probe` — capability probe for a slot's LLM route.
//!
//! Plain `health` answers "is the runtime process up". That is not the
//! question that bites: on 2026-08-28 a slot whose process was perfectly
//! healthy failed every real request because its route could no longer serve
//! a multi-turn tool call. The gateway knew; the CLI did not; the user found
//! out at their first prompt.
//!
//! This probe asks the route the one question a tool-using agent depends on:
//! *will you accept a conversation that already contains a tool call and its
//! result?* It sends that exact shape — assistant `tool_calls` followed by a
//! `tool` result — straight to the provider endpoint, and reports the
//! upstream status and message verbatim when the answer is no.
//!
//! Deliberately NOT routed through the runtime's `/v1/run`: driving a real
//! agent turn depends on the model choosing to call a tool, which is
//! nondeterministic and consumes a turn. Constructing the history ourselves
//! makes the probe a fixed, provider-agnostic assertion.

use std::time::Duration;

use serde_json::{json, Value};

/// Wire protocol of the probed endpoint. Mirrors `DiscoveredProvider::
/// api_protocol` — the tool-history shape differs between the two families,
/// so a single JSON body cannot serve both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeProtocol {
    /// `POST {base}/chat/completions` — OpenAI, ollama, switchAILocal, groq…
    OpenAiCompletions,
    /// `POST {base}/messages` — Anthropic's native API.
    AnthropicMessages,
}

/// Everything needed to reach a slot's route without starting the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTarget {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub protocol: ProbeProtocol,
    /// Env var holding the credential, and whether the endpoint refuses
    /// requests without one. Local endpoints accept a placeholder.
    pub key_env: &'static [&'static str],
    pub key_required: bool,
}

/// Name the probe advertises. Never executed — the point is that the route
/// accepts the *shape*, not that the tool exists.
const PROBE_TOOL: &str = "makakoo_capability_probe";

/// Resolve a model specifier (`provider/model`, or bare for DSH's
/// switchAILocal lock) into a reachable endpoint.
///
/// Pure: no I/O, no env reads, so the provider table is unit-testable.
pub fn resolve_target(model_spec: &str) -> anyhow::Result<ProbeTarget> {
    let spec = model_spec.trim();
    if spec.is_empty() {
        anyhow::bail!("slot has no model configured; nothing to probe");
    }
    let (provider, model) = match spec.split_once('/') {
        // A bare model id is the DSH contract: switchAILocal only.
        None => ("switchailocal", spec),
        Some((provider, "")) => {
            anyhow::bail!("model specifier '{spec}' names provider '{provider}' with no model")
        }
        Some((provider, model)) => (provider, model),
    };
    let (base_url, protocol, key_env, key_required): (&str, _, &'static [&'static str], bool) =
        match provider {
            // Order mirrors dsh_scaffold/runner.rs: DEEPSEEK_API_KEY wins,
            // then AIL_API_KEY. Probing with a different key than the runtime
            // uses turns a working slot into a 401.
            "switchailocal" => (
                "http://127.0.0.1:18080/v1",
                ProbeProtocol::OpenAiCompletions,
                &["DEEPSEEK_API_KEY", "AIL_API_KEY"],
                false,
            ),
            "ollama" => (
                "http://127.0.0.1:11434/v1",
                ProbeProtocol::OpenAiCompletions,
                &["OLLAMA_API_KEY"],
                false,
            ),
            "openai" => (
                "https://api.openai.com/v1",
                ProbeProtocol::OpenAiCompletions,
                &["OPENAI_API_KEY"],
                true,
            ),
            "anthropic" => (
                "https://api.anthropic.com/v1",
                ProbeProtocol::AnthropicMessages,
                &["ANTHROPIC_API_KEY"],
                true,
            ),
            other => anyhow::bail!(
                "cannot probe provider '{other}'; supported: switchailocal, ollama, openai, anthropic"
            ),
        };
    Ok(ProbeTarget {
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        protocol,
        key_env,
        key_required,
    })
}

/// Read the first of `names` that a `.env` file assigns a non-empty value.
///
/// Deliberately minimal — this is a credential lookup, not a dotenv
/// implementation. Kept pure so it can be tested without touching the
/// process environment, which any ambient key would otherwise shadow.
fn dotenv_lookup(contents: &str, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        contents.lines().find_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            if key.trim() != *name {
                return None;
            }
            let value = value.trim().trim_matches(['"', '\'']);
            (!value.is_empty()).then(|| value.to_string())
        })
    })
}

impl ProbeTarget {
    pub fn url(&self) -> String {
        let path = match self.protocol {
            ProbeProtocol::OpenAiCompletions => "chat/completions",
            ProbeProtocol::AnthropicMessages => "messages",
        };
        format!("{}/{}", self.base_url.trim_end_matches('/'), path)
    }

    /// A conversation that already contains one completed tool call. This is
    /// the shape that a route without multi-turn tool support rejects, and
    /// the shape every agent turn after the first one uses.
    pub fn body(&self) -> Value {
        match self.protocol {
            ProbeProtocol::OpenAiCompletions => json!({
                "model": self.model,
                // Minimal: the probe cares about acceptance, not output.
                "max_tokens": 16,
                "messages": [
                    {"role": "user", "content": "ping"},
                    {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_makakoo_probe",
                            "type": "function",
                            "function": {"name": PROBE_TOOL, "arguments": "{}"}
                        }]
                    },
                    {
                        "role": "tool",
                        "tool_call_id": "call_makakoo_probe",
                        "content": "pong"
                    }
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": PROBE_TOOL,
                        "description": "Capability probe. Never executed.",
                        "parameters": {"type": "object", "properties": {}}
                    }
                }]
            }),
            ProbeProtocol::AnthropicMessages => json!({
                "model": self.model,
                "max_tokens": 16,
                "tools": [{
                    "name": PROBE_TOOL,
                    "description": "Capability probe. Never executed.",
                    "input_schema": {"type": "object", "properties": {}}
                }],
                "messages": [
                    {"role": "user", "content": "ping"},
                    {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": "toolu_makakoo_probe",
                            "name": PROBE_TOOL,
                            "input": {}
                        }]
                    },
                    {
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "toolu_makakoo_probe",
                            "content": "pong"
                        }]
                    }
                ]
            }),
        }
    }

    /// Resolve the credential exactly the way the runtime does: process
    /// environment first, then the generated project's `.env` (which the
    /// runner loads via `node --env-file-if-exists=.env`). A slot whose key
    /// lives only in that file must not fail its probe with a 401.
    fn resolve_key(&self, project_dir: Option<&std::path::Path>) -> Option<String> {
        let from_env = self
            .key_env
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty());
        if from_env.is_some() {
            return from_env;
        }
        let contents = std::fs::read_to_string(project_dir?.join(".env")).ok()?;
        dotenv_lookup(&contents, self.key_env)
    }
}

/// Outcome of one probe. Separated from printing so the exit-code mapping is
/// testable without a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Supported,
    /// Route reachable and the request was refused on its merits — the
    /// endpoint will not serve this conversation shape. Carries the upstream
    /// words, because a paraphrase loses the one detail worth having.
    Rejected {
        status: u16,
        upstream: String,
    },
    /// Route reachable but the answer says nothing about capability: bad
    /// credentials, unknown model, rate limit, provider outage. Reporting
    /// these as "cannot serve tool calls" would send the user to fix the
    /// wrong thing entirely.
    Inconclusive {
        status: u16,
        reason: &'static str,
        upstream: String,
    },
    /// Could not reach the endpoint at all.
    Unreachable {
        detail: String,
    },
    /// Reached the endpoint, but it did not answer in time. Distinct from
    /// Unreachable because the cause and the fix are different: a local
    /// provider loading a cold model routinely takes over a minute, and
    /// calling that "unreachable" sends the user to debug a live endpoint.
    TimedOut {
        seconds: u64,
    },
    /// A required credential is absent; probing would report a misleading
    /// auth failure instead of a capability answer.
    MissingCredential {
        env: String,
    },
}

impl ProbeOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            ProbeOutcome::Supported => 0,
            _ => 1,
        }
    }

    /// True only when the probe actually answered the capability question.
    pub fn is_conclusive(&self) -> bool {
        matches!(
            self,
            ProbeOutcome::Supported | ProbeOutcome::Rejected { .. }
        )
    }

    /// How long to wait for an answer. A cold model load on a local provider
    /// was measured at 66s on 2026-08-28; a shorter budget turns a healthy
    /// route into a false failure.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(150);
}

/// Upstream error bodies can be long; keep enough to identify the cause.
const UPSTREAM_EXCERPT: usize = 600;

/// Hard ceiling on how much of an untrusted response we buffer at all. The
/// display excerpt alone is not a bound: it is applied after the whole body
/// is already in memory.
const UPSTREAM_READ_LIMIT: usize = 64 * 1024;

/// Render an upstream body for a terminal.
///
/// Two things must not survive: control characters (a response can carry ANSI
/// escapes that rewrite the surrounding output, and this text is printed
/// verbatim by design), and the credential itself (several providers echo the
/// offending key back in their error message).
fn excerpt(body: &str, secret: Option<&str>) -> String {
    let mut cleaned: String = body
        .trim()
        .chars()
        .map(|ch| {
            if ch == '\n' || ch == '\t' {
                ' '
            } else if ch.is_control() {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect();
    if let Some(secret) = secret.filter(|value| value.len() >= 8) {
        cleaned = cleaned.replace(secret, "***redacted***");
    }
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= UPSTREAM_EXCERPT {
        return cleaned.to_string();
    }
    let cut: String = cleaned.chars().take(UPSTREAM_EXCERPT).collect();
    format!("{cut}…")
}

/// Split "the route refused this shape" from "the route could not answer".
///
/// Only a request-level rejection is evidence about capability. Auth,
/// quota, missing-model and server faults say nothing about whether the
/// endpoint supports tool history — calling them a capability failure sends
/// the user to fix the wrong thing.
fn classify(status: u16) -> Result<(), &'static str> {
    match status {
        401 | 403 => Err("authentication rejected"),
        404 => Err("model or endpoint not found"),
        429 => Err("rate limited or out of quota"),
        402 => Err("billing or quota problem"),
        500..=599 => Err("provider-side failure"),
        _ => Ok(()),
    }
}

pub async fn run_probe(
    target: &ProbeTarget,
    project_dir: Option<&std::path::Path>,
    timeout: Duration,
) -> ProbeOutcome {
    let key = target.resolve_key(project_dir);
    if target.key_required && key.is_none() {
        return ProbeOutcome::MissingCredential {
            env: target.key_env.join(" or "),
        };
    }
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(error) => {
            return ProbeOutcome::Unreachable {
                detail: error.to_string(),
            }
        }
    };
    let mut request = client.post(target.url()).json(&target.body());
    request = match target.protocol {
        ProbeProtocol::OpenAiCompletions => request.header(
            "Authorization",
            // Local gateways accept any bearer; sending one keeps the
            // request shape identical across providers.
            format!(
                "Bearer {}",
                key.clone().unwrap_or_else(|| "makakoo-local".into())
            ),
        ),
        ProbeProtocol::AnthropicMessages => request
            .header("x-api-key", key.clone().unwrap_or_default())
            .header("anthropic-version", "2023-06-01"),
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return ProbeOutcome::TimedOut {
                seconds: timeout.as_secs(),
            }
        }
        Err(error) => {
            return ProbeOutcome::Unreachable {
                detail: transport_detail(&error),
            }
        }
    };
    let status = response.status();
    if status.is_success() {
        return ProbeOutcome::Supported;
    }
    // Bounded read: stop pulling bytes once we have more than we will ever
    // print, so an oversized error body cannot exhaust memory.
    let mut body = Vec::new();
    let mut response = response;
    while body.len() < UPSTREAM_READ_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) | Err(_) => break,
        }
    }
    body.truncate(UPSTREAM_READ_LIMIT);
    let upstream = excerpt(&String::from_utf8_lossy(&body), key.as_deref());
    match classify(status.as_u16()) {
        Ok(()) => ProbeOutcome::Rejected {
            status: status.as_u16(),
            upstream,
        },
        Err(reason) => ProbeOutcome::Inconclusive {
            status: status.as_u16(),
            reason,
            upstream,
        },
    }
}

/// reqwest's Display stops at "error sending request for url (…)", which
/// names the URL we already printed and nothing about the cause. Walk the
/// source chain for the part that identifies the failure.
fn transport_detail(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        let text = inner.to_string();
        if !parts.iter().any(|part| part == &text) {
            parts.push(text);
        }
        source = inner.source();
    }
    parts.join(": ")
}

/// Human report. Returns the process exit code.
pub fn report(slot_id: &str, target: &ProbeTarget, outcome: &ProbeOutcome) -> i32 {
    match outcome {
        ProbeOutcome::Supported => {
            println!(
                "{}: multi-turn tool calls supported ({} {} via {})",
                slot_id,
                target.provider,
                target.model,
                target.url()
            );
        }
        ProbeOutcome::Rejected { status, upstream } => {
            crate::output::print_error(format!(
                "{}: route cannot serve multi-turn tool calls — {} {} returned HTTP {}",
                slot_id, target.provider, target.model, status
            ));
            println!("upstream: {upstream}");
        }
        ProbeOutcome::Inconclusive {
            status,
            reason,
            upstream,
        } => {
            crate::output::print_error(format!(
                "{}: probe inconclusive — {} {} returned HTTP {} ({}); capability unknown",
                slot_id, target.provider, target.model, status, reason
            ));
            println!("upstream: {upstream}");
        }
        ProbeOutcome::Unreachable { detail } => {
            crate::output::print_error(format!(
                "{}: {} unreachable at {} ({})",
                slot_id,
                target.provider,
                target.url(),
                detail
            ));
        }
        ProbeOutcome::TimedOut { seconds } => {
            crate::output::print_error(format!(
                "{}: {} did not answer within {}s at {} — capability unknown",
                slot_id,
                target.provider,
                seconds,
                target.url()
            ));
            println!(
                "hint: a local provider loading a cold model can exceed this; retry once the model is resident"
            );
        }
        ProbeOutcome::MissingCredential { env } => {
            crate::output::print_error(format!(
                "{}: cannot probe {} without a credential; set {}",
                slot_id, target.provider, env
            ));
        }
    }
    outcome.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_model_resolves_to_the_dsh_switchailocal_lock() {
        let target = resolve_target("ail-compound").unwrap();
        assert_eq!(target.provider, "switchailocal");
        assert_eq!(target.model, "ail-compound");
        assert_eq!(target.url(), "http://127.0.0.1:18080/v1/chat/completions");
        assert!(
            !target.key_required,
            "local gateway must probe without a key"
        );
    }

    #[test]
    fn each_supported_provider_maps_to_its_own_endpoint_and_protocol() {
        for (spec, provider, url, protocol, required) in [
            (
                "ollama/qwen3:8b",
                "ollama",
                "http://127.0.0.1:11434/v1/chat/completions",
                ProbeProtocol::OpenAiCompletions,
                false,
            ),
            (
                "openai/gpt-5.5",
                "openai",
                "https://api.openai.com/v1/chat/completions",
                ProbeProtocol::OpenAiCompletions,
                true,
            ),
            (
                "anthropic/claude-sonnet-4-6",
                "anthropic",
                "https://api.anthropic.com/v1/messages",
                ProbeProtocol::AnthropicMessages,
                true,
            ),
        ] {
            let target = resolve_target(spec).unwrap();
            assert_eq!(target.provider, provider);
            assert_eq!(target.url(), url);
            assert_eq!(target.protocol, protocol);
            assert_eq!(target.key_required, required, "{spec}");
        }
    }

    #[test]
    fn unknown_or_malformed_specifiers_fail_with_the_supported_list() {
        let error = resolve_target("mistral/large").unwrap_err().to_string();
        assert!(error.contains("switchailocal"), "{error}");
        assert!(error.contains("anthropic"), "{error}");
        assert!(resolve_target("").is_err());
        assert!(
            resolve_target("openai/").is_err(),
            "a provider with no model is not probeable"
        );
    }

    #[test]
    fn openai_body_carries_a_completed_tool_call_and_its_result() {
        // This is the whole point of the probe: the request must already
        // contain tool history, otherwise it only tests plain chat and the
        // 2026-08-28 failure sails straight through.
        let body = resolve_target("switchailocal/ail-compound").unwrap().body();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        let call = &messages[1]["tool_calls"][0];
        assert_eq!(call["function"]["name"], PROBE_TOOL);
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], call["id"]);
        assert_eq!(body["tools"][0]["function"]["name"], PROBE_TOOL);
    }

    #[test]
    fn anthropic_body_uses_tool_use_and_tool_result_blocks() {
        let body = resolve_target("anthropic/claude-sonnet-4-6")
            .unwrap()
            .body();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(
            messages[2]["content"][0]["tool_use_id"],
            messages[1]["content"][0]["id"]
        );
        // Anthropic declares tools flat, not nested under "function".
        assert_eq!(body["tools"][0]["name"], PROBE_TOOL);
    }

    #[test]
    fn switchailocal_key_precedence_matches_the_generated_runner() {
        // runner.mjs: DEEPSEEK_API_KEY ?? AIL_API_KEY. Probing with the
        // other order can 401 a slot that works.
        let target = resolve_target("ail-compound").unwrap();
        assert_eq!(target.key_env, ["DEEPSEEK_API_KEY", "AIL_API_KEY"]);
    }

    #[test]
    fn credentials_can_come_from_the_projects_dotenv() {
        // The runner loads the generated .env via
        // `node --env-file-if-exists=.env`; a key that lives only there must
        // still reach the probe instead of producing a false 401.
        let contents = "# comment\nDSH_MAX_TOKENS=8192\nDEEPSEEK_API_KEY=\"sk-from-dotenv\"\n";
        let names = ["DEEPSEEK_API_KEY", "AIL_API_KEY"];
        assert_eq!(
            dotenv_lookup(contents, &names).as_deref(),
            Some("sk-from-dotenv")
        );
        // Precedence within the file follows the runner's order.
        let both = "AIL_API_KEY=second\nDEEPSEEK_API_KEY=first\n";
        assert_eq!(dotenv_lookup(both, &names).as_deref(), Some("first"));
        // Scaffolded .env files ship the key commented out and blank; neither
        // is a credential.
        assert!(dotenv_lookup("# DEEPSEEK_API_KEY=nope\n", &names).is_none());
        assert!(dotenv_lookup("DEEPSEEK_API_KEY=\n", &names).is_none());
        assert!(dotenv_lookup("DSH_MAX_TOKENS=8192\n", &names).is_none());
    }

    #[test]
    fn operational_failures_are_not_reported_as_capability_failures() {
        // A 401 says nothing about tool-history support. Blaming capability
        // for it sends the user to fix entirely the wrong thing.
        for status in [401, 403, 402, 404, 429, 500, 503] {
            assert!(
                classify(status).is_err(),
                "HTTP {status} must not be read as a capability answer"
            );
        }
        // A request-level rejection IS the capability answer — ollama returns
        // 400 "does not support tools", switchAILocal returns 422.
        for status in [400, 422] {
            assert!(
                classify(status).is_ok(),
                "HTTP {status} is about the request"
            );
        }
    }

    #[test]
    fn only_a_conclusive_outcome_answers_the_capability_question() {
        assert!(ProbeOutcome::Supported.is_conclusive());
        assert!(ProbeOutcome::Rejected {
            status: 422,
            upstream: String::new()
        }
        .is_conclusive());
        assert!(!ProbeOutcome::Inconclusive {
            status: 401,
            reason: "authentication rejected",
            upstream: String::new()
        }
        .is_conclusive());
        assert!(!ProbeOutcome::Unreachable {
            detail: String::new()
        }
        .is_conclusive());
    }

    #[test]
    fn upstream_text_cannot_inject_terminal_escapes_or_echo_the_key() {
        // Upstream bodies are printed verbatim by design, so an ANSI escape
        // in one would rewrite the surrounding output.
        let hostile = "error: \u{1b}[2Jcleared your screen\u{7}";
        let cleaned = excerpt(hostile, None);
        assert!(!cleaned.contains('\u{1b}'), "{cleaned}");
        assert!(!cleaned.contains('\u{7}'), "{cleaned}");
        assert!(cleaned.contains("cleared your screen"));

        // Several providers echo the offending key back in the error body.
        let leaky = "invalid api key: sk-secret-abcdef123456";
        let redacted = excerpt(leaky, Some("sk-secret-abcdef123456"));
        assert!(!redacted.contains("sk-secret-abcdef123456"), "{redacted}");
        assert!(redacted.contains("***redacted***"));

        // Newlines are preserved as spaces so the report stays one line.
        assert_eq!(excerpt("a\nb", None), "a b");
    }

    #[test]
    fn a_slow_answer_is_not_an_unreachable_endpoint() {
        // Measured 2026-08-28: a cold qwen3:8b load answered in 66s. The
        // first version of this probe used a 20s budget and reported a live
        // ollama as "unreachable".
        assert!(
            ProbeOutcome::DEFAULT_TIMEOUT.as_secs() >= 120,
            "timeout must survive a cold local model load"
        );
        let timed_out = ProbeOutcome::TimedOut { seconds: 150 };
        assert_eq!(timed_out.exit_code(), 1);
        assert!(
            !timed_out.is_conclusive(),
            "a timeout says nothing about capability"
        );
    }

    #[test]
    fn only_a_supported_route_exits_zero() {
        assert_eq!(ProbeOutcome::Supported.exit_code(), 0);
        for outcome in [
            ProbeOutcome::Rejected {
                status: 422,
                upstream: "no member satisfies chat_multiturn_tools".into(),
            },
            ProbeOutcome::Unreachable {
                detail: "connection refused".into(),
            },
            ProbeOutcome::MissingCredential {
                env: "OPENAI_API_KEY".into(),
            },
            ProbeOutcome::Inconclusive {
                status: 429,
                reason: "rate limited or out of quota",
                upstream: String::new(),
            },
            ProbeOutcome::TimedOut { seconds: 150 },
        ] {
            assert_eq!(outcome.exit_code(), 1, "{outcome:?} must fail the probe");
        }
    }

    #[test]
    fn upstream_excerpt_is_bounded_but_keeps_the_reason() {
        let long = format!("capability unavailable: {}", "x".repeat(5_000));
        let cut = excerpt(&long, None);
        assert!(cut.starts_with("capability unavailable:"));
        assert!(cut.chars().count() <= UPSTREAM_EXCERPT + 1);
        assert_eq!(excerpt("  short  ", None), "short");
    }
}
