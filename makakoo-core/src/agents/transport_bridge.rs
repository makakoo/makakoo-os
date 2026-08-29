//! Bridge between a slot's chat transports and its DSH runtime API.
//!
//! Everything this needs already existed and none of it was wired up:
//! `TelegramAdapter` polls `getUpdates` and sends replies, spec
//! `channels:` compile into slot `[[transport]]` entries, and the
//! generated runtime answers `POST /v1/run`. The missing piece was the
//! task that drains inbound frames and calls the runtime — this module.
//!
//!   telegram getUpdates ─▶ InboundSink ─▶ bridge ─▶ POST /v1/run
//!                                            └──── sendMessage ◀──┘
//!
//! Three properties are load-bearing:
//!
//!   * **Reply-only.** A frame is only ever produced in response to an
//!     inbound message. Nothing here can send unsolicited output.
//!   * **Deny by default.** An empty allowlist rejects every sender.
//!     A transport whose allowlist is empty is not started at all,
//!     because it could never do anything except log refusals.
//!   * **The bridge cannot kill a healthy gateway.** Transport tasks
//!     are supervised separately from the gateway child; an adapter
//!     that dies takes its own transport down and nothing else.
//!
//! The runtime binds an ephemeral port, so a respawned gateway comes
//! back somewhere else. The endpoint is therefore re-read from
//! `runtime.json` on every inbound message rather than cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::agents::runtime_client::{read_endpoint, run_prompt, RunOutcome};
use crate::agents::slot::{AgentRuntimeEngine, AgentSlot};
use crate::agents::supervisor::{RestartBudget, RestartDecision};
use crate::agents::supervisor_runtime::ShutdownSignal;
use crate::transport::config::{TransportConfig, TransportEntry};
use crate::transport::frame::{MakakooInboundFrame, MakakooOutboundFrame};
use crate::transport::gateway::Gateway;
use crate::transport::secrets::SecretsAdapter;
use crate::transport::telegram::TelegramAdapter;
use crate::transport::{Transport, TransportContext};
use crate::{MakakooError, Result};

/// Telegram caps `sendMessage` text at 4096. Whether that 4096 counts
/// Unicode characters or UTF-16 code units is not settled — tdlib's
/// length check reads as characters, while UTF-16 units are what the
/// Bot API uses for entity offsets, and an astral character is two of
/// them. Measuring in UTF-16 is the stricter of the two readings, so
/// it is correct under either: at worst it splits emoji-heavy text
/// earlier than strictly necessary.
const CHUNK_UTF16: usize = 3500;

/// Beyond this many chunks the reply is truncated. A runaway answer
/// should not turn into a flood of messages.
const MAX_CHUNKS: usize = 8;

/// How long one `/v1/run` may take. The runtime serialises per session
/// and enforces its own limits; this is the outer bound.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// Inbound frames buffered while a run is in flight.
const INBOUND_QUEUE: usize = 64;

/// Messages buffered per conversation. Beyond this the sender is told
/// the agent is busy, which is information; silently queueing an hour
/// of work is not.
const SESSION_QUEUE: usize = 8;

/// Runs that may be in flight at once, across all conversations. The
/// generated runtime admits 4 concurrent runs by default
/// (`MAKAKOO_DSH_MAX_CONCURRENT`) and queues the rest, so going wider
/// here only moves the wait.
///
/// This bounds *work*, not conversations. Bounding conversations was a
/// mistake: a worker that is merely idle holds no resource worth
/// rationing, and counting it made four past chats lock every future
/// one out.
const MAX_CONCURRENT_RUNS: usize = 4;

/// Live per-conversation workers. An idle one costs a task and a small
/// queue, so this is a memory bound, not a concurrency bound.
const MAX_SESSION_WORKERS: usize = 64;

/// A worker with nothing to do for this long retires, freeing its slot
/// for a conversation that does.
const WORKER_IDLE: Duration = Duration::from_secs(300);

/// Minimum gap between "busy" notices to one conversation, so a sender
/// who keeps typing cannot make the bot answer itself in a loop.
const BUSY_NOTICE_COOLDOWN: Duration = Duration::from_secs(30);

/// How long an inbound message waits for `runtime.json` to appear. The
/// transports come up before the gateway child, so the very first
/// message of a slot's life can race the runtime's TCP bind.
const ENDPOINT_WAIT: Duration = Duration::from_secs(30);
const ENDPOINT_POLL: Duration = Duration::from_millis(500);

/// Error text relayed into a chat is capped — the full detail goes to
/// the supervisor log.
const RELAYED_ERROR_CHARS: usize = 400;

/// The runtime's session-id charset: `^[A-Za-z0-9._:-]{1,128}$`.
const SESSION_ID_MAX: usize = 128;

fn session_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')
}

/// Stable session id for one conversation. Messages that share it
/// share the agent's memory, so what goes into it decides who can see
/// whose context. Four components, each of which would otherwise merge
/// two conversations that are not the same conversation:
///
///   * **transport** — two bots on one slot polling chats that happen
///     to share an id are different conversations.
///   * **chat** — the conversation itself.
///   * **thread** — a forum topic is its own conversation; without
///     this, every topic in a group collapses into one session.
///   * **sender, when the chat is not a 1:1** — in a private chat
///     Telegram sets `chat.id == from.id`, so a differing pair means
///     a group, where one member's session must not carry another
///     member's context.
///
/// Stability matters as much as separation: the id must survive a
/// restart, so it is derived only from values Telegram repeats.
pub fn session_id_for(
    transport_id: &str,
    conversation_id: &str,
    thread_id: Option<&str>,
    sender_id: &str,
) -> String {
    let mut raw = format!("tg:{transport_id}:{conversation_id}");
    if let Some(thread) = thread_id {
        raw.push_str(&format!(":t{thread}"));
    }
    if sender_id != conversation_id {
        raw.push_str(&format!(":u{sender_id}"));
    }
    // The readable form is only safe when no component can forge the
    // delimiter: a transport id of `a:b` with chat `c` would otherwise
    // render identically to transport `a` with chat `b:c` — two
    // different conversations sharing one memory. Telegram ids are
    // numeric, so this is the guard for a caller that is not Telegram.
    let forgeable = [transport_id, conversation_id, sender_id]
        .iter()
        .chain(thread_id.iter())
        .any(|part| part.contains(':'));
    if !forgeable && raw.len() <= SESSION_ID_MAX && raw.chars().all(session_char_ok) {
        return raw;
    }
    // Fall back to a hash of a length-prefixed encoding, which is
    // injective by construction and still deterministic — which is
    // what session persistence actually needs.
    let mut canonical = String::new();
    for part in [
        transport_id,
        conversation_id,
        thread_id.unwrap_or_default(),
        sender_id,
    ] {
        canonical.push_str(&format!("{}:{}", part.len(), part));
    }
    // `None` and `Some("")` are different conversations.
    canonical.push(if thread_id.is_some() { 'T' } else { 'N' });
    let digest = blake3::hash(canonical.as_bytes()).to_hex();
    format!("tg:h{}", &digest[..40])
}

/// The **senders** a transport will accept.
///
/// The ACL is checked against the message's sender id, so the two
/// lists unioned here are the two places an operator can name a
/// sender: the transport-level `allowed_users`, and Telegram's
/// `allowed_chat_ids` (in a private chat, the chat id *is* the user
/// id). `agent create` fills both from a spec's `channels:`;
/// hand-written TOML often sets only the config block. Empty across
/// both means deny-all.
///
/// `allowed_group_ids` is deliberately NOT unioned in. A group id is
/// negative and can never equal a member's sender id, so folding it in
/// would authorize nobody in that group — except a `from`-less
/// anonymous admin post, whose sender id falls back to the chat id.
/// Granting "anonymous posts only" to an operator who asked for "this
/// group" is worse than refusing: see [`ignored_group_ids`].
pub fn effective_allowlist(entry: &TransportEntry) -> Vec<String> {
    let mut out = entry.allowed_users.clone();
    if let TransportConfig::Telegram(cfg) = &entry.config {
        out.extend(cfg.allowed_chat_ids.iter().cloned());
    }
    out.retain(|v| !v.trim().is_empty());
    out.sort();
    out.dedup();
    out
}

/// Group ids the operator configured that this version cannot honor.
/// Reported so a group-scoped allowlist never looks like it is in
/// force when it is not.
/// Conversations this transport may address without being spoken to
/// first.
///
/// `allowed_chat_ids` are genuine chat ids, so they are the correct
/// destination set. `allowed_users` are *sender* ids; for a private
/// chat Telegram makes the two numerically equal, which is why they are
/// a sound fallback for the common one-to-one bot, but they are not a
/// destination authorisation in general.
pub fn delivery_destinations(entry: &TransportEntry) -> Vec<String> {
    let mut out = match &entry.config {
        TransportConfig::Telegram(cfg) if !cfg.allowed_chat_ids.is_empty() => {
            cfg.allowed_chat_ids.clone()
        }
        _ => entry.allowed_users.clone(),
    };
    out.retain(|v| !v.trim().is_empty());
    out.sort();
    out.dedup();
    out
}

pub fn ignored_group_ids(entry: &TransportEntry) -> Vec<String> {
    match &entry.config {
        TransportConfig::Telegram(cfg) => cfg
            .allowed_group_ids
            .iter()
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

/// A transport that will not be started, and why. Surfaced by
/// `agent start` so a misconfiguration is loud instead of silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedTransport {
    pub transport_id: String,
    pub reason: String,
}

/// What `plan` decided: the transports to run, and the ones it refused.
pub struct BridgePlan {
    pub bridge: Option<TransportBridge>,
    pub skipped: Vec<SkippedTransport>,
}

/// One configured, credential-bearing telegram transport.
pub struct BridgedTransport {
    adapter: Arc<TelegramAdapter>,
    /// Inbound SENDER allowlist — who may talk to the agent.
    allowed: Arc<Vec<String>>,
    /// Outbound DESTINATION list — which conversations the agent may
    /// open unprompted. Deliberately separate from `allowed`: a user id
    /// permitted to post in a group is not the same thing as a chat
    /// authorised to receive a scheduled report, and using one as the
    /// other would send a group's briefing into a private DM.
    destinations: Arc<Vec<String>>,
}

impl BridgedTransport {
    pub fn transport_id(&self) -> &str {
        self.adapter.transport_id()
    }

    /// `getMe`, classified. The distinction matters: a token Telegram
    /// rejects will be rejected again on every retry, while an
    /// unreachable API is a condition the poll loop recovers from on
    /// its own. Treating both as fatal would take a working bot
    /// offline for the duration of a network blip.
    pub async fn check(&self) -> CredentialCheck {
        let transport_id = self.transport_id().to_string();
        match self.adapter.verify_credentials().await {
            Ok(_) => CredentialCheck::Ok { transport_id },
            Err(MakakooError::Http(error)) => CredentialCheck::Unreachable {
                transport_id,
                detail: self.redact(&error.to_string()),
            },
            Err(error) => CredentialCheck::Rejected {
                transport_id,
                detail: self.redact(&error.to_string()),
            },
        }
    }

    /// The Telegram API puts the bot token in the URL path, and an
    /// HTTP error's Display carries the URL. Anything derived from one
    /// goes through here before it reaches a log, a terminal, or a
    /// chat.
    fn redact(&self, text: &str) -> String {
        redact_secret(text, &self.adapter.bot_token)
    }
}

/// Replace every occurrence of `secret` with a marker. A short or
/// empty secret is ignored rather than turning the whole string into
/// markers.
/// A cheap, cloneable handle for pushing an *unsolicited* message
/// through an already-planned transport.
///
/// Scheduled ticks have no inbound frame to reply to, so they cannot
/// reuse the request/response path. Delivery goes to
/// [`delivery_destinations`] — the transport's configured chat ids,
/// falling back to the sender allowlist only where Telegram makes the
/// two identical. It is NOT the inbound sender ACL.
#[derive(Clone)]
pub struct DeliveryTarget {
    adapter: Arc<TelegramAdapter>,
    chats: Arc<Vec<String>>,
}

impl DeliveryTarget {
    pub fn transport_id(&self) -> &str {
        self.adapter.transport_id()
    }

    /// Authorised destinations for unsolicited sends.
    pub fn chats(&self) -> &[String] {
        &self.chats
    }

    /// True when this transport has nowhere authorised to deliver.
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty()
    }

    /// Send `text` to one chat, chunked to Telegram's message limit.
    /// Returns the first send error; earlier chunks may already have
    /// been delivered.
    pub async fn send_text(&self, chat_id: &str, text: &str) -> Result<()> {
        for chunk in split_for_telegram(text) {
            let out = MakakooOutboundFrame {
                transport_id: self.adapter.transport_id().to_string(),
                transport_kind: "telegram".to_string(),
                conversation_id: chat_id.to_string(),
                thread_id: None,
                thread_kind: None,
                text: chunk,
                // Nothing to reply to — the agent started this exchange.
                reply_to_message_id: None,
            };
            self.adapter.send(&out).await.map_err(|e| {
                MakakooError::Config(redact_secret(&e.to_string(), &self.adapter.bot_token))
            })?;
        }
        Ok(())
    }
}

pub fn redact_secret(text: &str, secret: &str) -> String {
    if secret.len() < 8 {
        return text.to_string();
    }
    text.replace(secret, "<bot-token-redacted>")
}

/// Outcome of a transport's credential check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialCheck {
    /// Telegram accepted the token.
    Ok { transport_id: String },
    /// Telegram answered and refused the token. Retrying cannot help.
    Rejected {
        transport_id: String,
        detail: String,
    },
    /// The API could not be reached. The poll loop retries.
    Unreachable {
        transport_id: String,
        detail: String,
    },
}

impl CredentialCheck {
    pub fn transport_id(&self) -> &str {
        match self {
            Self::Ok { transport_id }
            | Self::Rejected { transport_id, .. }
            | Self::Unreachable { transport_id, .. } => transport_id,
        }
    }

    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }

    /// Operator-facing one-liner.
    pub fn describe(&self) -> String {
        match self {
            Self::Ok { transport_id } => {
                format!("transport '{transport_id}': credentials accepted")
            }
            Self::Rejected {
                transport_id,
                detail,
            } => format!("transport '{transport_id}': bot token rejected — {detail}"),
            Self::Unreachable {
                transport_id,
                detail,
            } => format!("transport '{transport_id}': Telegram unreachable — {detail}"),
        }
    }
}

/// The set of transports attached to one slot's runtime.
pub struct TransportBridge {
    slot_id: String,
    project_dir: PathBuf,
    transports: Vec<BridgedTransport>,
    http: reqwest::Client,
}

impl TransportBridge {
    /// Decide what to run for `slot`. Returns `bridge: None` when the
    /// slot has no eligible transport — a legacy gateway slot, or one
    /// with no enabled telegram block. Errors only on a problem the
    /// operator must fix before the slot can start at all.
    pub fn plan(slot: &AgentSlot, secrets: &dyn SecretsAdapter) -> Result<BridgePlan> {
        Self::plan_with_api_base(slot, secrets, None)
    }

    /// `plan`, with the Telegram API base overridden (tests).
    pub fn plan_with_api_base(
        slot: &AgentSlot,
        secrets: &dyn SecretsAdapter,
        api_base: Option<&str>,
    ) -> Result<BridgePlan> {
        let mut skipped = Vec::new();
        let candidates: Vec<&TransportEntry> = slot
            .transports
            .iter()
            .filter(|t| t.enabled && t.kind == "telegram")
            .collect();
        for entry in slot.transports.iter().filter(|t| t.enabled) {
            if entry.kind != "telegram" {
                skipped.push(SkippedTransport {
                    transport_id: entry.id.clone(),
                    reason: format!(
                        "kind '{}' has no supervisor-hosted adapter yet — only telegram is bridged",
                        entry.kind
                    ),
                });
            }
        }
        if candidates.is_empty() {
            return Ok(BridgePlan {
                bridge: None,
                skipped,
            });
        }

        let Some(runtime) = slot.runtime.as_ref() else {
            // Legacy slots are served by the Python harveychat gateway,
            // which owns its own ingress. Bridging them too would
            // double-answer every message.
            skipped.extend(candidates.iter().map(|entry| SkippedTransport {
                transport_id: entry.id.clone(),
                reason:
                    "slot has no compiled runtime; its legacy gateway owns channel ingress".into(),
            }));
            return Ok(BridgePlan {
                bridge: None,
                skipped,
            });
        };
        if runtime.engine != AgentRuntimeEngine::DeepseekHarness {
            skipped.extend(candidates.iter().map(|entry| SkippedTransport {
                transport_id: entry.id.clone(),
                reason: format!("runtime engine '{}' has no /v1/run API", runtime.engine),
            }));
            return Ok(BridgePlan {
                bridge: None,
                skipped,
            });
        }

        let mut transports = Vec::new();
        for entry in candidates {
            let ignored = ignored_group_ids(entry);
            if !ignored.is_empty() {
                skipped.push(SkippedTransport {
                    transport_id: entry.id.clone(),
                    reason: format!(
                        "allowed_group_ids ({}) are ignored — the allowlist matches message \
                         senders, and a group id never equals a member's sender id; list the \
                         members in allowed_users instead",
                        ignored.join(", ")
                    ),
                });
            }
            let allowed = effective_allowlist(entry);
            if allowed.is_empty() {
                // Deny-all is the adapter's default, so starting this
                // would poll Telegram forever and refuse every message.
                skipped.push(SkippedTransport {
                    transport_id: entry.id.clone(),
                    reason: "allowlist is empty — every inbound message would be rejected; set \
                             allowed_users (or config.allowed_chat_ids)"
                        .into(),
                });
                continue;
            }
            let token = secrets.resolve(&entry.bot_token_ref()).map_err(|_| {
                // The resolver's own message only says "nothing
                // resolved"; what the operator needs is where to put
                // the token for THIS transport.
                let mut sources = Vec::new();
                if let Some(env) = entry.secret_env.as_deref() {
                    sources.push(format!("set ${env}"));
                }
                if let Some(key) = entry.secret_ref.as_deref() {
                    sources.push(format!("run `makakoo secret set {key}`"));
                }
                if sources.is_empty() {
                    sources.push(
                        "add secret_env or secret_ref to the [[transport]] block".to_string(),
                    );
                }
                MakakooError::Config(format!(
                    "transport '{}' of slot '{}': bot token not found — {}",
                    entry.id,
                    slot.slot_id,
                    sources.join(", or ")
                ))
            })?;
            let TransportConfig::Telegram(cfg) = &entry.config else {
                return Err(MakakooError::Config(format!(
                    "transport '{}' declares kind=telegram with a non-telegram [config] body",
                    entry.id
                )));
            };
            let ctx = TransportContext {
                slot_id: slot.slot_id.clone(),
                transport_id: entry.id.clone(),
            };
            let adapter = match api_base {
                Some(base) => TelegramAdapter::with_api_base(
                    ctx,
                    cfg.clone(),
                    token.value,
                    allowed.clone(),
                    base.to_string(),
                ),
                None => TelegramAdapter::new(ctx, cfg.clone(), token.value, allowed.clone()),
            };
            transports.push(BridgedTransport {
                adapter: Arc::new(adapter),
                allowed: Arc::new(allowed),
                destinations: Arc::new(delivery_destinations(entry)),
            });
        }

        if transports.is_empty() {
            return Ok(BridgePlan {
                bridge: None,
                skipped,
            });
        }
        Ok(BridgePlan {
            bridge: Some(TransportBridge {
                slot_id: slot.slot_id.clone(),
                project_dir: runtime.project_dir.clone(),
                transports,
                http: reqwest::Client::builder()
                    .timeout(RUN_TIMEOUT)
                    .build()
                    .map_err(|e| MakakooError::Config(format!("build http client: {e}")))?,
            }),
            skipped,
        })
    }

    /// Handles for delivering scheduled output. Cloneable and cheap;
    /// taken before `verify_and_spawn` consumes the bridge.
    pub fn delivery_targets(&self) -> Vec<DeliveryTarget> {
        self.transports
            .iter()
            .map(|t| DeliveryTarget {
                adapter: t.adapter.clone(),
                chats: t.destinations.clone(),
            })
            .collect()
    }

    pub fn transport_ids(&self) -> Vec<String> {
        self.transports
            .iter()
            .map(|t| t.transport_id().to_string())
            .collect()
    }

    /// Check every transport's credentials without starting anything.
    /// `agent start` uses this so a wrong token is reported to the
    /// operator's terminal instead of only to the supervisor log.
    pub async fn check_credentials(&self) -> Vec<CredentialCheck> {
        let mut out = Vec::with_capacity(self.transports.len());
        for transport in &self.transports {
            out.push(transport.check().await);
        }
        out
    }

    /// Check credentials, then spawn a listener + dispatch task per
    /// surviving transport. Returns the task handles and the
    /// transports that were refused.
    ///
    /// A rejected token is fatal for that transport only; everything
    /// else — the other transports, and the gateway child — is
    /// unaffected.
    pub async fn verify_and_spawn(
        self,
        shutdown: ShutdownSignal,
    ) -> (Vec<JoinHandle<()>>, Vec<SkippedTransport>) {
        let TransportBridge {
            slot_id,
            project_dir,
            transports,
            http,
        } = self;
        let mut handles = Vec::new();
        let mut refused = Vec::new();
        for transport in transports {
            // Each check is a network round trip with a 60s client
            // timeout. Racing shutdown keeps a stop issued during
            // startup from looking wedged behind them.
            let mut interrupt = shutdown.clone();
            let outcome = tokio::select! {
                outcome = transport.check() => outcome,
                _ = interrupt.wait() => return (handles, refused),
            };
            match outcome {
                CredentialCheck::Rejected {
                    transport_id,
                    detail,
                } => {
                    refused.push(SkippedTransport {
                        transport_id,
                        reason: format!("bot token rejected — {detail}"),
                    });
                }
                check => {
                    if let CredentialCheck::Unreachable { detail, .. } = &check {
                        tracing::warn!(
                            target: "makakoo_core::agents::transport_bridge",
                            transport_id = transport.transport_id(),
                            detail = detail,
                            "could not verify credentials at startup — the poll loop will retry"
                        );
                    }
                    handles.push(tokio::spawn(run_transport(
                        slot_id.clone(),
                        project_dir.clone(),
                        transport,
                        http.clone(),
                        shutdown.clone(),
                    )));
                }
            }
        }
        (handles, refused)
    }
}

/// Poll one transport and answer its messages until shutdown.
async fn run_transport(
    slot_id: String,
    project_dir: PathBuf,
    transport: BridgedTransport,
    http: reqwest::Client,
    mut shutdown: ShutdownSignal,
) {
    let BridgedTransport {
        adapter,
        allowed,
        destinations: _,
    } = transport;
    let transport_id = adapter.transport_id().to_string();
    let (tx, mut rx) = mpsc::channel::<MakakooInboundFrame>(INBOUND_QUEUE);

    let listener = {
        let adapter = adapter.clone();
        let mut shutdown = shutdown.clone();
        let transport_id = transport_id.clone();
        tokio::spawn(async move {
            // The adapter absorbs transient poll errors itself, so a
            // returned Err means the listener gave up — a rejected
            // token, or a sink that closed. Restart it on the same
            // budget the gateway uses, so a permanently broken
            // transport eventually stops retrying instead of logging
            // forever. The gateway child is never affected either way.
            let mut budget = RestartBudget::new();
            loop {
                let outcome = tokio::select! {
                    result = adapter.start(tx.clone()) => result,
                    _ = shutdown.wait() => return,
                };
                match outcome {
                    Ok(()) => return,
                    Err(error) => {
                        tracing::error!(
                            target: "makakoo_core::agents::transport_bridge",
                            transport_id = transport_id,
                            error = redact_secret(&error.to_string(), &adapter.bot_token),
                            "telegram listener exited"
                        );
                        match budget.record_crash(chrono::Utc::now()) {
                            RestartDecision::Backoff(delay) => {
                                tokio::select! {
                                    _ = tokio::time::sleep(delay) => {}
                                    _ = shutdown.wait() => return,
                                }
                            }
                            RestartDecision::CircuitBreak => {
                                tracing::error!(
                                    target: "makakoo_core::agents::transport_bridge",
                                    transport_id = transport_id,
                                    "telegram listener failed too often — this transport stays \
                                     down until the slot is restarted"
                                );
                                return;
                            }
                        }
                    }
                }
            }
        })
    };

    tracing::info!(
        target: "makakoo_core::agents::transport_bridge",
        slot = slot_id,
        transport_id = transport_id,
        allowed_senders = allowed.len(),
        "telegram transport bridged to the slot runtime"
    );

    // One worker per conversation. Handling every frame on this loop
    // would mean a single 10-minute run blocks every other chat the
    // bot serves; separate workers keep one conversation's messages in
    // order without letting it stall the others.
    let mut router = SessionRouter::new(
        slot_id.clone(),
        project_dir.clone(),
        adapter.clone(),
        http.clone(),
        shutdown.clone(),
    );
    loop {
        tokio::select! {
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                // The ACL is checked before anything is queued, so an
                // unknown sender cannot occupy a worker slot.
                if !allowed.iter().any(|u| u == &frame.sender_id) {
                    tracing::warn!(
                        target: "makakoo_core::agents::transport_bridge",
                        slot = slot_id,
                        transport_id = frame.transport_id,
                        sender_id = frame.sender_id,
                        "inbound from a non-allowlisted sender reached the bridge — dropped"
                    );
                    continue;
                }
                router.route(frame).await;
            }
            _ = shutdown.wait() => break,
        }
    }
    listener.abort();
    let _ = listener.await;
    router.drain().await;
}

/// Keeps one worker per live conversation and hands each frame to its
/// own. Everything here runs on the transport's single routing task,
/// so nothing in it may await a network call.
struct SessionRouter {
    slot_id: String,
    project_dir: PathBuf,
    adapter: Arc<TelegramAdapter>,
    http: reqwest::Client,
    shutdown: ShutdownSignal,
    /// Bounds runs in flight across every conversation.
    permits: Arc<tokio::sync::Semaphore>,
    workers: HashMap<String, SessionWorker>,
    /// Last time each conversation was told we are busy, so the notice
    /// cannot be amplified into a reply per inbound message.
    busy_notices: HashMap<String, tokio::time::Instant>,
    /// Fire-and-forget outbound notices, so a stalled `sendMessage`
    /// cannot block routing for every other conversation.
    notices: tokio::task::JoinSet<()>,
}

/// A per-conversation worker: its queue, and the task draining it.
struct SessionWorker {
    tx: mpsc::Sender<MakakooInboundFrame>,
    task: JoinHandle<()>,
}

impl SessionRouter {
    fn new(
        slot_id: String,
        project_dir: PathBuf,
        adapter: Arc<TelegramAdapter>,
        http: reqwest::Client,
        shutdown: ShutdownSignal,
    ) -> Self {
        Self {
            slot_id,
            project_dir,
            adapter,
            http,
            shutdown,
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS)),
            workers: HashMap::new(),
            busy_notices: HashMap::new(),
            notices: tokio::task::JoinSet::new(),
        }
    }

    /// Deliver one authorised frame to its conversation's worker.
    async fn route(&mut self, frame: MakakooInboundFrame) {
        let session_id = session_id_for(
            &frame.transport_id,
            &frame.conversation_id,
            frame.thread_id.as_deref(),
            &frame.sender_id,
        );
        self.reap();

        // One retry: a worker can retire between the sweep and the
        // send, and dropping the message for that would be a bug the
        // sender experiences as silence.
        let mut frame = frame;
        for attempt in 0..2 {
            if !self.workers.contains_key(&session_id) {
                if self.workers.len() >= MAX_SESSION_WORKERS {
                    self.notify_busy(
                        &frame,
                        "⚠️ the agent is handling too many conversations right now — try again \
                         shortly",
                    );
                    return;
                }
                self.spawn_worker(session_id.clone());
            }
            let worker = self.workers.get(&session_id).expect("just inserted");
            match worker.tx.try_send(frame) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    self.notify_busy(
                        &returned,
                        "⚠️ too many pending messages in this conversation — wait for the \
                         current answer",
                    );
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(returned)) => {
                    // The worker retired underneath us. Reap it and,
                    // on the first attempt, start a fresh one.
                    if let Some(worker) = self.workers.remove(&session_id) {
                        worker.task.abort();
                    }
                    if attempt == 1 {
                        tracing::warn!(
                            target: "makakoo_core::agents::transport_bridge",
                            slot = self.slot_id,
                            session = session_id,
                            "could not hand a message to a session worker — dropped"
                        );
                        return;
                    }
                    frame = returned;
                }
            }
        }
    }

    /// Forget workers that have retired.
    fn reap(&mut self) {
        self.workers.retain(|_, worker| !worker.task.is_finished());
        while self.notices.try_join_next().is_some() {}
        let now = tokio::time::Instant::now();
        self.busy_notices
            .retain(|_, at| now.duration_since(*at) < BUSY_NOTICE_COOLDOWN);
    }

    fn spawn_worker(&mut self, session_id: String) {
        let (tx, mut rx) = mpsc::channel::<MakakooInboundFrame>(SESSION_QUEUE);
        let slot_id = self.slot_id.clone();
        let project_dir = self.project_dir.clone();
        let adapter = self.adapter.clone();
        let http = self.http.clone();
        let permits = self.permits.clone();
        let mut shutdown = self.shutdown.clone();
        let worker_session = session_id.clone();
        let task = tokio::spawn(async move {
            loop {
                // An idle worker retires rather than holding a slot
                // forever — the queue is owned by the router, so
                // `recv()` alone would never return.
                let frame = tokio::select! {
                    received = tokio::time::timeout(WORKER_IDLE, rx.recv()) => match received {
                        Ok(Some(frame)) => frame,
                        Ok(None) | Err(_) => return,
                    },
                    _ = shutdown.wait() => return,
                };
                // Conversations are independent, but the total number
                // of runs is not: this is what keeps the runtime from
                // being swamped.
                let permit = tokio::select! {
                    permit = permits.clone().acquire_owned() => match permit {
                        Ok(permit) => permit,
                        Err(_) => return,
                    },
                    _ = shutdown.wait() => return,
                };
                // A run can take minutes, so shutdown races it —
                // otherwise `agent stop` would wait for the model.
                tokio::select! {
                    _ = handle_frame(&slot_id, &project_dir, &adapter, &http, &worker_session, frame) => {}
                    _ = shutdown.wait() => {
                        tracing::info!(
                            target: "makakoo_core::agents::transport_bridge",
                            slot = slot_id,
                            session = worker_session,
                            "shutdown during an in-flight run — the reply is abandoned"
                        );
                        return;
                    }
                }
                drop(permit);
            }
        });
        self.workers.insert(session_id, SessionWorker { tx, task });
    }

    /// Tell a sender we cannot take their message — at most once per
    /// conversation per cooldown, and never on the routing task's own
    /// timeline.
    fn notify_busy(&mut self, frame: &MakakooInboundFrame, text: &str) {
        let now = tokio::time::Instant::now();
        if let Some(sent) = self.busy_notices.get(&frame.conversation_id) {
            if now.duration_since(*sent) < BUSY_NOTICE_COOLDOWN {
                return;
            }
        }
        self.busy_notices.insert(frame.conversation_id.clone(), now);
        let adapter = self.adapter.clone();
        let out = MakakooOutboundFrame {
            transport_id: frame.transport_id.clone(),
            transport_kind: frame.transport_kind.clone(),
            conversation_id: frame.conversation_id.clone(),
            thread_id: frame.thread_id.clone(),
            thread_kind: frame.thread_kind.clone(),
            text: text.to_string(),
            reply_to_message_id: Some(frame.message_id.clone()),
        };
        self.notices.spawn(async move {
            if let Err(error) = adapter.send(&out).await {
                tracing::warn!(
                    target: "makakoo_core::agents::transport_bridge",
                    transport_id = out.transport_id,
                    error = redact_secret(&error.to_string(), &adapter.bot_token),
                    "could not deliver a busy notice"
                );
            }
        });
    }

    /// Shut every worker down. Called once the routing loop has exited.
    async fn drain(mut self) {
        self.notices.abort_all();
        let workers = std::mem::take(&mut self.workers);
        // Dropping the senders lets a worker blocked on `recv()` exit;
        // the ones mid-run observe `shutdown` themselves.
        let tasks: Vec<JoinHandle<()>> = workers
            .into_values()
            .map(|worker| {
                drop(worker.tx);
                worker.task
            })
            .collect();
        for task in tasks {
            let mut task = std::pin::pin!(task);
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        while self.notices.join_next().await.is_some() {}
    }
}

/// Answer one inbound message. The sender was authorised, and the
/// session id fixed, by `dispatch_frame` before this was queued.
async fn handle_frame(
    slot_id: &str,
    project_dir: &Path,
    adapter: &Arc<TelegramAdapter>,
    http: &reqwest::Client,
    session_id: &str,
    frame: MakakooInboundFrame,
) {
    let outcome = match wait_for_endpoint(project_dir, slot_id, ENDPOINT_WAIT).await {
        Ok(endpoint) => run_prompt(http, &endpoint, &frame.text, session_id, RUN_TIMEOUT).await,
        Err(error) => Err(error),
    };
    let reply = reply_text(&outcome);
    if let Err(error) = &outcome {
        tracing::error!(
            target: "makakoo_core::agents::transport_bridge",
            slot = slot_id,
            transport_id = frame.transport_id,
            session = session_id,
            error = %error,
            "slot runtime call failed — relaying the error to the chat"
        );
    }

    for (index, chunk) in split_for_telegram(&reply).into_iter().enumerate() {
        let out = MakakooOutboundFrame {
            transport_id: frame.transport_id.clone(),
            transport_kind: frame.transport_kind.clone(),
            conversation_id: frame.conversation_id.clone(),
            thread_id: frame.thread_id.clone(),
            thread_kind: frame.thread_kind.clone(),
            text: chunk,
            // Only the first chunk anchors to the question; the rest
            // are continuations of our own reply.
            reply_to_message_id: (index == 0).then(|| frame.message_id.clone()),
        };
        if let Err(error) = adapter.send(&out).await {
            tracing::error!(
                target: "makakoo_core::agents::transport_bridge",
                slot = slot_id,
                transport_id = frame.transport_id,
                error = redact_secret(&error.to_string(), &adapter.bot_token),
                "telegram sendMessage failed — dropping the rest of this reply"
            );
            break;
        }
    }
}

/// Read the runtime endpoint, retrying while the runtime is still
/// coming up. Returns the last error once `budget` is spent.
///
/// The port is ephemeral, so this re-reads `runtime.json` per message
/// instead of caching: a respawned gateway binds a different port.
async fn wait_for_endpoint(
    project_dir: &Path,
    slot_id: &str,
    budget: Duration,
) -> Result<crate::agents::runtime_client::RuntimeEndpoint> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        match read_endpoint(project_dir, slot_id) {
            Ok(endpoint) => return Ok(endpoint),
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tokio::time::sleep(ENDPOINT_POLL).await;
            }
        }
    }
}

/// Turn a run result into the text the chat should receive. Every
/// branch produces something: silence would read as a hung bot.
pub fn reply_text(outcome: &Result<RunOutcome>) -> String {
    match outcome {
        Ok(RunOutcome::Answer(answer)) if !answer.trim().is_empty() => answer.clone(),
        Ok(RunOutcome::Answer(_)) => "⚠️ the agent returned an empty answer".to_string(),
        Ok(RunOutcome::Refused { status, message }) => {
            format!("⚠️ agent runtime error ({status}): {}", clip(message))
        }
        Err(error) => format!("⚠️ agent runtime unreachable: {}", clip(&error.to_string())),
    }
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= RELAYED_ERROR_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(RELAYED_ERROR_CHARS).collect();
    format!("{cut}…")
}

/// Length in the unit Telegram enforces.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Byte offset just past the last character that fits in `budget`
/// UTF-16 units. Always lands on a character boundary.
fn utf16_cut(text: &str, budget: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if units + width > budget {
            return offset;
        }
        units += width;
    }
    text.len()
}

/// Split a reply into Telegram-sized messages, preferring a line break
/// near the limit so code blocks and lists survive the cut.
pub fn split_for_telegram(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec!["⚠️ the agent returned an empty answer".to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if utf16_len(rest) <= CHUNK_UTF16 {
            chunks.push(rest.to_string());
            rest = "";
            break;
        }
        if chunks.len() + 1 == MAX_CHUNKS {
            break;
        }
        let hard = utf16_cut(rest, CHUNK_UTF16);
        // Prefer the last newline in the back half of the chunk.
        let cut = rest[..hard]
            .rfind('\n')
            .map(|i| i + 1)
            .filter(|i| *i > hard / 2)
            .unwrap_or(hard);
        // `hard` is never 0 for a non-empty `rest` (one character
        // always fits in a 3500-unit budget), so this always advances.
        chunks.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start_matches('\n');
    }
    if !rest.is_empty() {
        chunks.push(format!(
            "… reply truncated after {MAX_CHUNKS} messages ({} characters dropped)",
            rest.chars().count()
        ));
    }
    chunks.retain(|c| !c.is_empty());
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::slot::AgentRuntime;
    use crate::agents::supervisor_runtime::shutdown_pair;
    use crate::transport::config::TelegramConfig;
    use crate::transport::secrets::tests::MemSecrets;
    use std::path::PathBuf;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The shared fixture deliberately declares NO `secret_env`: env
    /// vars are process-global, so a transport that reads one would
    /// pick up whatever a concurrently-running test happened to set.
    fn telegram_entry(id: &str, allowed: Vec<&str>) -> TransportEntry {
        telegram_entry_inner(id, allowed, None)
    }

    /// Same fixture, bound to a named env var. Every caller must use a
    /// name unique to its own test.
    fn telegram_entry_with_env(id: &str, allowed: Vec<&str>, env: &str) -> TransportEntry {
        telegram_entry_inner(id, allowed, Some(env.to_string()))
    }

    fn telegram_entry_inner(
        id: &str,
        allowed: Vec<&str>,
        secret_env: Option<String>,
    ) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: Some("agent/secretary/telegram/bot_token".into()),
            secret_env,
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: allowed.into_iter().map(String::from).collect(),
            config: TransportConfig::Telegram(TelegramConfig {
                polling_timeout_seconds: 1,
                allowed_chat_ids: vec![],
                allowed_group_ids: vec![],
                support_thread: false,
            }),
        }
    }

    fn slot_with(transports: Vec<TransportEntry>, project_dir: PathBuf) -> AgentSlot {
        AgentSlot {
            slot_id: "secretary".into(),
            name: "secretary".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: "supervised_pair".into(),
            transports,
            llm: None,
            runtime: Some(AgentRuntime {
                engine: AgentRuntimeEngine::DeepseekHarness,
                project_dir,
            }),
            triggers: Vec::new(),
        }
    }

    fn secrets() -> MemSecrets {
        MemSecrets::with(&[("agent/secretary/telegram/bot_token", "TEST-BOT-TOKEN")])
    }

    // ── pure helpers ──────────────────────────────────────────────

    #[test]
    fn a_private_chat_gets_one_stable_session() {
        // Private chat: Telegram sets chat.id == from.id, so no sender
        // component is added and the id stays short and readable.
        let id = session_id_for("telegram-main", "746496145", None, "746496145");
        assert_eq!(id, "tg:telegram-main:746496145");
        // Same inputs, same id — sessions survive a restart.
        assert_eq!(
            id,
            session_id_for("telegram-main", "746496145", None, "746496145")
        );
    }

    #[test]
    fn two_members_of_one_group_do_not_share_a_session() {
        let group = "-1001234567";
        let a = session_id_for("telegram-main", group, None, "111");
        let b = session_id_for("telegram-main", group, None, "222");
        assert_ne!(a, b, "group members must not see each other's context");
        assert!(a.contains(group) && a.ends_with("u111"));
    }

    #[test]
    fn forum_topics_are_separate_conversations() {
        let group = "-1001234567";
        let topic_a = session_id_for("telegram-main", group, Some("5"), "111");
        let topic_b = session_id_for("telegram-main", group, Some("9"), "111");
        let no_topic = session_id_for("telegram-main", group, None, "111");
        assert_ne!(topic_a, topic_b);
        assert_ne!(topic_a, no_topic);
    }

    #[test]
    fn two_bots_on_one_slot_do_not_share_a_session() {
        let a = session_id_for("telegram-main", "1", None, "1");
        let b = session_id_for("telegram-alt", "1", None, "1");
        assert_ne!(a, b);
    }

    #[test]
    fn a_component_cannot_forge_the_delimiter_to_merge_two_conversations() {
        // Rendered naively, both of these would be "tg:a:b:c".
        let a = session_id_for("a:b", "c", None, "c");
        let b = session_id_for("a", "b:c", None, "b:c");
        assert_ne!(a, b, "two different conversations must not share a session");
        // And `None` thread is not the same conversation as `Some("")`.
        assert_ne!(
            session_id_for("a:b", "c", None, "c"),
            session_id_for("a:b", "c", Some(""), "c")
        );
    }

    #[test]
    fn every_session_id_satisfies_the_runtime_charset_and_length() {
        let exotic = "a".repeat(400);
        let cases = [
            session_id_for("telegram-main", "746496145", None, "746496145"),
            session_id_for("telegram-main", "-1001234567", Some("42"), "111"),
            session_id_for(&exotic, "-1001234567", Some("42"), "111"),
            session_id_for("weird id/with spaces+plus", "1", None, "2"),
        ];
        for id in cases {
            assert!(!id.is_empty() && id.len() <= 128, "{id}");
            assert!(id.chars().all(session_char_ok), "{id}");
            assert!(id != "." && id != "..", "{id}");
        }
        // The hashed fallback is still deterministic.
        assert_eq!(
            session_id_for(&exotic, "1", None, "2"),
            session_id_for(&exotic, "1", None, "2")
        );
    }

    #[test]
    fn allowlist_unions_both_places_a_sender_can_be_named() {
        let mut entry = telegram_entry("telegram-main", vec!["100", " "]);
        entry.config = TransportConfig::Telegram(TelegramConfig {
            polling_timeout_seconds: 30,
            allowed_chat_ids: vec!["100".into(), "200".into()],
            allowed_group_ids: vec!["-300".into()],
            support_thread: false,
        });
        // The group id is NOT a sender id and must not silently widen
        // the ACL — it would only ever match an anonymous post.
        assert_eq!(effective_allowlist(&entry), vec!["100", "200"]);
        assert_eq!(ignored_group_ids(&entry), vec!["-300"]);
    }

    #[test]
    fn a_group_only_allowlist_does_not_start_and_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = telegram_entry("telegram-main", vec![]);
        entry.config = TransportConfig::Telegram(TelegramConfig {
            polling_timeout_seconds: 30,
            allowed_chat_ids: vec![],
            allowed_group_ids: vec!["-1001234567".into()],
            support_thread: false,
        });
        let slot = slot_with(vec![entry], tmp.path().to_path_buf());
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_none());
        let reasons: Vec<&str> = plan.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert!(
            reasons.iter().any(|r| r.contains("allowed_group_ids")),
            "{reasons:?}"
        );
        assert!(
            reasons.iter().any(|r| r.contains("allowlist is empty")),
            "{reasons:?}"
        );
    }

    #[test]
    fn allowlist_is_empty_when_nothing_is_configured() {
        let entry = telegram_entry("telegram-main", vec![]);
        assert!(effective_allowlist(&entry).is_empty());
    }

    #[test]
    fn short_replies_are_one_message() {
        assert_eq!(split_for_telegram("hello"), vec!["hello"]);
    }

    #[test]
    fn empty_replies_never_produce_silence() {
        assert_eq!(split_for_telegram("   ").len(), 1);
        assert!(split_for_telegram("")[0].contains("empty answer"));
    }

    #[test]
    fn long_replies_split_below_the_telegram_limit() {
        let body = "line of text\n".repeat(1000);
        let chunks = split_for_telegram(&body);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(utf16_len(chunk) <= CHUNK_UTF16, "{}", utf16_len(chunk));
        }
    }

    #[test]
    fn splitting_prefers_a_line_break() {
        let body = format!("{}\n{}", "a".repeat(3400), "b".repeat(3400));
        let chunks = split_for_telegram(&body);
        assert_eq!(chunks[0], "a".repeat(3400));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn multibyte_replies_split_on_character_boundaries() {
        // Pure-emoji body: a byte-indexed split would panic here.
        let body = "🙂".repeat(9000);
        let chunks = split_for_telegram(&body);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() * 2 == utf16_len(chunk) || chunk.contains("truncated"));
        }
    }

    #[test]
    fn astral_text_is_measured_the_way_telegram_measures_it() {
        // Telegram's 4096 limit counts UTF-16 code units. An emoji is
        // two, so a chunk sized in `char`s would be ~7000 units and
        // sendMessage would reject it outright.
        for body in ["🙂".repeat(9000), "𝕏".repeat(9000), "a".repeat(9000)] {
            for chunk in split_for_telegram(&body) {
                assert!(
                    utf16_len(&chunk) <= 4096,
                    "chunk is {} UTF-16 units — Telegram caps at 4096",
                    utf16_len(&chunk)
                );
            }
        }
    }

    #[test]
    fn every_chunk_advances_so_the_splitter_cannot_spin() {
        // A body with no newline and a single leading newline both
        // exercise the "no good break point" path.
        for body in ["z".repeat(40_000), format!("\n{}", "z".repeat(40_000))] {
            let chunks = split_for_telegram(&body);
            assert!(!chunks.is_empty());
            assert!(chunks.len() <= MAX_CHUNKS);
        }
    }

    #[test]
    fn a_bot_token_never_survives_into_relayed_text() {
        let token = "8123456789:AAH-fake-telegram-bot-token-value";
        let leaked = format!(
            "error sending request for url (https://api.telegram.org/bot{token}/getMe): timed out"
        );
        let clean = redact_secret(&leaked, token);
        assert!(!clean.contains(token), "{clean}");
        assert!(clean.contains("<bot-token-redacted>"), "{clean}");
        // A too-short "secret" must not blank out unrelated text.
        assert_eq!(redact_secret("abc def", "abc"), "abc def");
    }

    #[test]
    fn runaway_replies_are_truncated_with_a_visible_marker() {
        let body = "x".repeat(CHUNK_UTF16 * (MAX_CHUNKS + 4));
        let chunks = split_for_telegram(&body);
        assert_eq!(chunks.len(), MAX_CHUNKS);
        assert!(chunks.last().unwrap().contains("truncated"));
    }

    #[test]
    fn every_run_outcome_produces_text() {
        assert_eq!(
            reply_text(&Ok(RunOutcome::Answer("hi".into()))),
            "hi".to_string()
        );
        assert!(reply_text(&Ok(RunOutcome::Answer("  ".into()))).contains("empty answer"));
        let refused = reply_text(&Ok(RunOutcome::Refused {
            status: 502,
            message: "upstream failed".into(),
        }));
        assert!(refused.contains("502") && refused.contains("upstream failed"));
        let failed = reply_text(&Err(MakakooError::Config("no runtime.json".into())));
        assert!(failed.contains("unreachable") && failed.contains("no runtime.json"));
    }

    #[test]
    fn relayed_errors_are_clipped() {
        let long = "e".repeat(5000);
        let text = reply_text(&Err(MakakooError::Config(long)));
        assert!(text.chars().count() < 600, "{}", text.chars().count());
        assert!(text.ends_with('…'));
    }

    // ── planning ──────────────────────────────────────────────────

    #[test]
    fn plan_refuses_a_transport_with_an_empty_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec![])],
            tmp.path().to_path_buf(),
        );
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_none(), "deny-all transport must not start");
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].reason.contains("allowlist is empty"));
    }

    #[test]
    fn plan_builds_a_bridge_for_an_allowlisted_telegram_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["746496145"])],
            tmp.path().to_path_buf(),
        );
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        let bridge = plan.bridge.expect("bridge");
        assert_eq!(bridge.transport_ids(), vec!["telegram-main"]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_errors_when_the_bot_token_cannot_be_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry_with_env(
                "telegram-main",
                vec!["746496145"],
                "MAKAKOO_TEST_TG_TOKEN_NEVER_SET",
            )],
            tmp.path().to_path_buf(),
        );
        let error = match TransportBridge::plan(&slot, &MemSecrets::with(&[])) {
            Err(error) => error,
            Ok(_) => panic!("an unresolvable bot token must fail planning"),
        };
        let text = error.to_string();
        assert!(text.contains("telegram-main"), "{text}");
        assert!(
            text.contains("set $MAKAKOO_TEST_TG_TOKEN_NEVER_SET"),
            "the error must name the env var to set: {text}"
        );
        assert!(
            text.contains("makakoo secret set"),
            "the error must name the keyring alternative: {text}"
        );
        assert_eq!(
            text.matches("config error").count(),
            1,
            "the message must not stack error prefixes: {text}"
        );
    }

    #[test]
    fn plan_skips_a_legacy_slot_so_its_python_gateway_stays_the_only_listener() {
        let mut slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["746496145"])],
            PathBuf::from("/unused"),
        );
        slot.runtime = None;
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_none());
        assert!(plan.skipped[0].reason.contains("legacy gateway"));
    }

    #[test]
    fn plan_skips_a_flue_slot_which_has_no_run_api() {
        let tmp = tempfile::tempdir().unwrap();
        let mut slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["746496145"])],
            tmp.path().to_path_buf(),
        );
        slot.runtime = Some(AgentRuntime {
            engine: AgentRuntimeEngine::Flue,
            project_dir: tmp.path().to_path_buf(),
        });
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_none());
        assert!(plan.skipped[0].reason.contains("no /v1/run"));
    }

    #[test]
    fn plan_reports_a_non_telegram_transport_as_unbridged() {
        let tmp = tempfile::tempdir().unwrap();
        let mut slack = telegram_entry("slack-main", vec!["U1"]);
        slack.kind = "slack".into();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"]), slack],
            tmp.path().to_path_buf(),
        );
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_some());
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].transport_id, "slack-main");
    }

    #[test]
    fn plan_ignores_a_disabled_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = telegram_entry("telegram-main", vec!["1"]);
        entry.enabled = false;
        let slot = slot_with(vec![entry], tmp.path().to_path_buf());
        let plan = TransportBridge::plan(&slot, &secrets()).unwrap();
        assert!(plan.bridge.is_none());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_resolves_the_token_from_the_environment_first() {
        // T4.3: the launchd plist sources ~/.env with `set -a`, so the
        // token arrives as a process env var. That path must win.
        // Unique to this test — see `telegram_entry`.
        let key = "MAKAKOO_TEST_TG_TOKEN_ENV_FIRST";
        std::env::set_var(key, "env-token");
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry_with_env("telegram-main", vec!["1"], key)],
            tmp.path().to_path_buf(),
        );
        let plan = TransportBridge::plan(&slot, &MemSecrets::with(&[])).unwrap();
        std::env::remove_var(key);
        assert!(
            plan.bridge.is_some(),
            "an env-only token must be enough to start"
        );
    }

    // ── credential verification ───────────────────────────────────

    #[tokio::test]
    async fn credential_check_flags_a_rejected_token() {
        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false, "description": "Unauthorized"
            })))
            .mount(&telegram)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"])],
            tmp.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let checks = plan.bridge.unwrap().check_credentials().await;
        assert_eq!(checks.len(), 1);
        assert!(checks[0].is_rejected(), "{:?}", checks[0]);
        let text = checks[0].describe();
        assert!(text.contains("telegram-main"), "{text}");
        assert!(text.contains("Unauthorized"), "{text}");
    }

    // ── full loop: telegram → runtime → telegram ──────────────────

    /// Stand up a mock Telegram API that serves exactly one update,
    /// then long-polls empty, and a mock DSH runtime with a live
    /// `runtime.json` pointing at it.
    struct Harness {
        telegram: MockServer,
        runtime: MockServer,
        project: tempfile::TempDir,
    }

    impl Harness {
        async fn new(sender_id: i64, runtime_response: ResponseTemplate) -> Self {
            let telegram = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/botTEST-BOT-TOKEN/getMe"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "result": {"id": 999, "username": "SecretaryBot"}
                })))
                .mount(&telegram)
                .await;
            // First poll delivers the message; later polls (higher
            // offset) return nothing.
            Mock::given(method("GET"))
                .and(path("/botTEST-BOT-TOKEN/getUpdates"))
                .and(wiremock::matchers::query_param("offset", "0"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true,
                    "result": [{
                        "update_id": 7,
                        "message": {
                            "message_id": 11,
                            "date": 1714123456,
                            "chat": {"id": 746496145, "type": "private"},
                            "from": {"id": sender_id, "username": "sebastian"},
                            "text": "what is on my calendar?"
                        }
                    }]
                })))
                .mount(&telegram)
                .await;
            Mock::given(method("GET"))
                .and(path("/botTEST-BOT-TOKEN/getUpdates"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"ok": true, "result": []}))
                        .set_delay(Duration::from_millis(200)),
                )
                .mount(&telegram)
                .await;
            Mock::given(method("POST"))
                .and(path("/botTEST-BOT-TOKEN/sendMessage"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"ok": true, "result": {}})),
                )
                .mount(&telegram)
                .await;

            let runtime = MockServer::start().await;
            // The bridge confirms the listener is this slot's runtime
            // before sending anything to it.
            Mock::given(method("GET"))
                .and(path("/health"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "slot": "secretary", "engine": "deepseek-harness"
                })))
                .mount(&runtime)
                .await;
            Mock::given(method("POST"))
                .and(path("/v1/run"))
                .respond_with(runtime_response)
                .mount(&runtime)
                .await;

            let project = tempfile::tempdir().unwrap();
            let token_file = project.path().join(".runtime-token");
            std::fs::write(&token_file, "runtime-secret\n").unwrap();
            let port: u16 = runtime.uri().rsplit(':').next().unwrap().parse().unwrap();
            std::fs::write(
                project.path().join("runtime.json"),
                serde_json::json!({
                    "slot": "secretary",
                    "engine": "deepseek-harness",
                    "host": "127.0.0.1",
                    "port": port,
                    "pid": std::process::id(),
                    "token_file": token_file,
                })
                .to_string(),
            )
            .unwrap();
            Self {
                telegram,
                runtime,
                project,
            }
        }

        async fn sends(&self) -> Vec<wiremock::Request> {
            self.telegram
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|r| r.url.path().ends_with("/sendMessage"))
                .collect()
        }

        async fn poll_count(&self) -> usize {
            self.telegram
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path().ends_with("/getUpdates"))
                .count()
        }

        /// Run the bridge until `ready` holds, then shut it down. The
        /// condition is what makes this deterministic — a fixed sleep
        /// would be a coin flip on a loaded machine.
        async fn run_until<F, Fut>(&self, allowed: Vec<&str>, ready: F)
        where
            F: Fn() -> Fut,
            Fut: std::future::Future<Output = bool>,
        {
            let slot = slot_with(
                vec![telegram_entry("telegram-main", allowed)],
                self.project.path().to_path_buf(),
            );
            let plan =
                TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&self.telegram.uri()))
                    .unwrap();
            let (trigger, signal) = shutdown_pair();
            let (handles, refused) = plan.bridge.expect("bridge").verify_and_spawn(signal).await;
            assert!(refused.is_empty(), "{refused:?}");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            while tokio::time::Instant::now() < deadline && !ready().await {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            trigger.fire();
            for handle in handles {
                tokio::time::timeout(Duration::from_secs(5), handle)
                    .await
                    .expect("bridge task must exit on shutdown")
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn an_allowlisted_message_is_answered_by_the_runtime() {
        let harness = Harness::new(
            746496145,
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"session_id": "tg:746496145", "response": "nothing today"}),
            ),
        )
        .await;
        harness
            .run_until(vec!["746496145"], || async {
                !harness.sends().await.is_empty()
            })
            .await;
        let sends = harness.sends().await;
        assert_eq!(sends.len(), 1, "expected exactly one reply");
        let body: serde_json::Value = serde_json::from_slice(&sends[0].body).unwrap();
        assert_eq!(body["text"], "nothing today");
        assert_eq!(body["chat_id"], 746496145_i64);
        assert_eq!(body["reply_to_message_id"], 11);

        // The runtime saw the stable per-chat session id.
        let runs: Vec<_> = harness
            .runtime
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/v1/run")
            .collect();
        assert_eq!(runs.len(), 1);
        let run: serde_json::Value = serde_json::from_slice(&runs[0].body).unwrap();
        assert_eq!(run["session_id"], "tg:telegram-main:746496145");
        assert_eq!(run["text"], "what is on my calendar?");
        assert_eq!(
            runs[0].headers.get("authorization").unwrap(),
            "Bearer runtime-secret"
        );
    }

    #[tokio::test]
    async fn a_runtime_failure_is_reported_into_the_chat() {
        let harness = Harness::new(
            746496145,
            ResponseTemplate::new(502)
                .set_body_json(serde_json::json!({"error": "provider refused the request"})),
        )
        .await;
        harness
            .run_until(vec!["746496145"], || async {
                !harness.sends().await.is_empty()
            })
            .await;
        let sends = harness.sends().await;
        assert_eq!(sends.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&sends[0].body).unwrap();
        let text = body["text"].as_str().unwrap();
        assert!(text.contains("502"), "{text}");
        assert!(text.contains("provider refused the request"), "{text}");
    }

    #[tokio::test]
    async fn a_stranger_gets_no_reply_and_never_reaches_the_runtime() {
        let harness = Harness::new(
            111222333,
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"session_id": "x", "response": "leaked"})),
        )
        .await;
        // Allowlist names someone else entirely. Waiting for a second
        // poll proves the update was consumed and a full cycle passed,
        // rather than asserting on a sleep that may have been too short.
        harness
            .run_until(vec!["746496145"], || async {
                harness.poll_count().await >= 2
            })
            .await;
        assert!(
            harness.sends().await.is_empty(),
            "a non-allowlisted sender must get nothing"
        );
        assert!(
            harness
                .runtime
                .received_requests()
                .await
                .unwrap()
                .is_empty(),
            "a non-allowlisted message must never reach the agent"
        );
    }

    #[tokio::test]
    async fn a_slow_conversation_does_not_block_a_different_one() {
        use wiremock::matchers::body_string_contains;

        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"id": 999}
            })))
            .mount(&telegram)
            .await;
        // Both messages arrive in one batch, slow one first.
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getUpdates"))
            .and(wiremock::matchers::query_param("offset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": [
                    {"update_id": 1, "message": {
                        "message_id": 1, "date": 1, "chat": {"id": 111, "type": "private"},
                        "from": {"id": 111}, "text": "slow"}},
                    {"update_id": 2, "message": {
                        "message_id": 2, "date": 1, "chat": {"id": 222, "type": "private"},
                        "from": {"id": 222}, "text": "fast"}}
                ]
            })))
            .mount(&telegram)
            .await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getUpdates"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": []}))
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&telegram)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST-BOT-TOKEN/sendMessage"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": {}})),
            )
            .mount(&telegram)
            .await;

        let runtime = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "slot": "secretary", "engine": "deepseek-harness"
            })))
            .mount(&runtime)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .and(body_string_contains("slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"session_id": "a", "response": "slow done"}))
                    // Far longer than the wait below, so a loaded
                    // machine cannot make a serial bridge look
                    // concurrent by finishing the slow run in time.
                    .set_delay(Duration::from_secs(20)),
            )
            .mount(&runtime)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .and(body_string_contains("fast"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"session_id": "b", "response": "fast done"})),
            )
            .mount(&runtime)
            .await;

        let project = tempfile::tempdir().unwrap();
        let token_file = project.path().join(".runtime-token");
        std::fs::write(&token_file, "runtime-secret\n").unwrap();
        let port: u16 = runtime.uri().rsplit(':').next().unwrap().parse().unwrap();
        std::fs::write(
            project.path().join("runtime.json"),
            serde_json::json!({
                "slot": "secretary", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": port,
                "pid": std::process::id(), "token_file": token_file,
            })
            .to_string(),
        )
        .unwrap();

        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["111", "222"])],
            project.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let (trigger, signal) = shutdown_pair();
        let (handles, _) = plan.bridge.unwrap().verify_and_spawn(signal).await;

        // Wait for the fast reply. Serial handling could not produce
        // one until the 20s slow run finished, so an answer inside this
        // window is only possible if the two conversations ran
        // independently. The window is generous so a loaded CI machine
        // does not fail a correct implementation.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let mut sends = Vec::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
            sends = telegram
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|r| r.url.path().ends_with("/sendMessage"))
                .collect();
            if !sends.is_empty() {
                break;
            }
        }
        trigger.fire();
        for handle in handles {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        assert_eq!(
            sends.len(),
            1,
            "expected the fast conversation to be answered while the slow one ran"
        );
        let body: serde_json::Value = serde_json::from_slice(&sends[0].body).unwrap();
        assert_eq!(body["chat_id"], 222_i64);
        assert_eq!(body["text"], "fast done");
    }

    /// Regression for a defect this bridge shipped in review: workers
    /// were capped at the concurrency limit and never retired, because
    /// the router owned their queues so `recv()` could not return
    /// `None`. Four conversations used once made every later
    /// conversation permanently "busy".
    #[tokio::test]
    async fn conversations_beyond_the_concurrency_limit_are_still_served() {
        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"id": 999}
            })))
            .mount(&telegram)
            .await;
        // Eight distinct chats — twice MAX_CONCURRENT_RUNS — in one
        // batch, each answered immediately.
        let senders: Vec<i64> = (1..=8).map(|n| 1000 + n).collect();
        let updates: Vec<serde_json::Value> = senders
            .iter()
            .enumerate()
            .map(|(i, id)| {
                serde_json::json!({
                    "update_id": i + 1,
                    "message": {
                        "message_id": i + 1, "date": 1,
                        "chat": {"id": id, "type": "private"},
                        "from": {"id": id}, "text": "ping"
                    }
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getUpdates"))
            .and(wiremock::matchers::query_param("offset", "0"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": updates})),
            )
            .mount(&telegram)
            .await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getUpdates"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": []}))
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&telegram)
            .await;
        Mock::given(method("POST"))
            .and(path("/botTEST-BOT-TOKEN/sendMessage"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": {}})),
            )
            .mount(&telegram)
            .await;

        let runtime = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "slot": "secretary", "engine": "deepseek-harness"
            })))
            .mount(&runtime)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"session_id": "s", "response": "pong"})),
            )
            .mount(&runtime)
            .await;

        let project = tempfile::tempdir().unwrap();
        let token_file = project.path().join(".runtime-token");
        std::fs::write(&token_file, "runtime-secret\n").unwrap();
        let port: u16 = runtime.uri().rsplit(':').next().unwrap().parse().unwrap();
        std::fs::write(
            project.path().join("runtime.json"),
            serde_json::json!({
                "slot": "secretary", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": port,
                "pid": std::process::id(), "token_file": token_file,
            })
            .to_string(),
        )
        .unwrap();

        let allowed: Vec<String> = senders.iter().map(|id| id.to_string()).collect();
        let slot = slot_with(
            vec![telegram_entry(
                "telegram-main",
                allowed.iter().map(String::as_str).collect(),
            )],
            project.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let (trigger, signal) = shutdown_pair();
        let (handles, _) = plan.bridge.unwrap().verify_and_spawn(signal).await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut sends: Vec<wiremock::Request> = Vec::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
            sends = telegram
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|r| r.url.path().ends_with("/sendMessage"))
                .collect();
            if sends.len() >= senders.len() {
                break;
            }
        }
        trigger.fire();
        for handle in handles {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        let answered: std::collections::HashSet<i64> = sends
            .iter()
            .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
            .filter(|b| b["text"] == "pong")
            .filter_map(|b| b["chat_id"].as_i64())
            .collect();
        for id in &senders {
            assert!(
                answered.contains(id),
                "chat {id} never got an answer — served {}/{} conversations",
                answered.len(),
                senders.len()
            );
        }
    }

    #[tokio::test]
    async fn the_bridge_drains_on_shutdown_without_any_traffic() {
        // No update is ever served, so the loop is idle; shutdown must
        // still return promptly.
        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"id": 999}
            })))
            .mount(&telegram)
            .await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getUpdates"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "result": []}))
                    .set_delay(Duration::from_secs(30)),
            )
            .mount(&telegram)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"])],
            tmp.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let (trigger, signal) = shutdown_pair();
        let (handles, refused) = plan.bridge.unwrap().verify_and_spawn(signal).await;
        assert!(refused.is_empty(), "{refused:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.fire();
        for handle in handles {
            tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("a long-poll in flight must not delay shutdown")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn credential_check_accepts_a_good_token() {
        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"id": 42, "username": "SecretaryBot"}
            })))
            .mount(&telegram)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"])],
            tmp.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let checks = plan.bridge.unwrap().check_credentials().await;
        assert_eq!(
            checks,
            vec![CredentialCheck::Ok {
                transport_id: "telegram-main".into()
            }]
        );
    }

    #[tokio::test]
    async fn an_unreachable_api_is_not_reported_as_a_bad_token() {
        // Nothing is listening on this port. A dead network must not
        // look like a credential the operator has to go fix.
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"])],
            tmp.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some("http://127.0.0.1:1"))
                .unwrap();
        let checks = plan.bridge.unwrap().check_credentials().await;
        assert!(
            matches!(checks[0], CredentialCheck::Unreachable { .. }),
            "{:?}",
            checks[0]
        );
        assert!(!checks[0].is_rejected());
    }

    #[tokio::test]
    async fn a_rejected_token_stops_its_transport_and_nothing_else() {
        let telegram = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/botTEST-BOT-TOKEN/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false, "description": "Unauthorized"
            })))
            .mount(&telegram)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let slot = slot_with(
            vec![telegram_entry("telegram-main", vec!["1"])],
            tmp.path().to_path_buf(),
        );
        let plan =
            TransportBridge::plan_with_api_base(&slot, &secrets(), Some(&telegram.uri())).unwrap();
        let (_trigger, signal) = shutdown_pair();
        let (handles, refused) = plan.bridge.unwrap().verify_and_spawn(signal).await;
        assert!(handles.is_empty(), "a rejected transport must not poll");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].transport_id, "telegram-main");
        assert!(refused[0].reason.contains("rejected"));
    }
}
