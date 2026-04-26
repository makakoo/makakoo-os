//! makakoo-mcp — stdio JSON-RPC MCP server binary.
//!
//! The spine for every `harvey_*` tool the MCP plugin surfaces to Claude
//! Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, and Qwen Code.
//! Wave 4 tool handlers (T13/T14/T15) plug into [`handlers::register_all`]
//! to flesh out the 41 Python-parity tools.
//!
//! # Wire protocol
//!
//! Newline-delimited JSON-RPC 2.0 over stdin/stdout. Confirmed against
//! the Python reference (`core/mcp/harvey_mcp.py`) and T1 acceptance
//! tests (`tests/migration/test_mcp_framing.py`). See `framing.rs` for
//! the malformed-JSON gotcha and the blank-line tolerance contract.
//!
//! # Logging
//!
//! All tracing output goes to **stderr** — stdout is reserved for the
//! JSON-RPC stream. Any stray stdout write will crash real MCP clients.
//!
//! # CLI flags
//!
//! ```text
//! makakoo-mcp                # run the stdio event loop
//! makakoo-mcp --health       # print {"ok":true,"tools":N} and exit
//! makakoo-mcp --list-tools   # print the tools/list descriptor JSON and exit
//! ```

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

mod dispatch;
mod framing;
mod handlers;
mod http_server;
mod jsonrpc;
mod server;
mod slack_events;
mod web_chat_ws;
mod webhook_router;
mod whatsapp_webhook;

use dispatch::{ToolContext, ToolRegistry};

/// CLI arguments.
#[derive(Parser, Debug)]
#[command(name = "makakoo-mcp", version, about = "Makakoo OS MCP stdio server")]
struct Args {
    /// Print `{"ok":true,"tools":N}` and exit. Used by smoke tests and
    /// the `harvey doctor` diagnostic.
    #[arg(long)]
    health: bool,

    /// Print the full `tools/list` descriptor array as pretty JSON and
    /// exit. Used by `harvey mcp list-tools` and by Wave 4 integration
    /// testing to confirm all 41 handlers are registered.
    #[arg(long)]
    list_tools: bool,

    /// v0.6 Phase B — run as an HTTP server instead of the stdio loop.
    /// Value is the address:port to bind (default 127.0.0.1:0 picks a
    /// random port). Signed-request auth (Ed25519) is mandatory in this
    /// mode; there is no unauthenticated network MCP path.
    #[arg(long, value_name = "ADDR:PORT")]
    http: Option<String>,

    /// Override the default bind interface (127.0.0.1). Only consulted
    /// when --http is set. Binding to a non-loopback address prints a
    /// warning banner; auth is still enforced.
    #[arg(long, value_name = "IP", default_value = "127.0.0.1")]
    bind: std::net::IpAddr,

    /// Path to the trust file (list of peer pubkeys authorized to call).
    /// Default `$MAKAKOO_HOME/config/peers/trusted.keys`. Lines of the
    /// form `<peer-name> <base64-pubkey>`.
    #[arg(long, value_name = "PATH")]
    trust_file: Option<std::path::PathBuf>,

    /// Path to this server's Ed25519 signing key. Default
    /// `$MAKAKOO_HOME/config/peers/signing.key`. Auto-generated on
    /// first run if absent (pubkey printed to stderr; share with peers).
    #[arg(long, value_name = "PATH")]
    signing_key: Option<std::path::PathBuf>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // Logs to stderr ALWAYS. stdout is the wire. Format is controlled
    // by $MAKAKOO_LOG_FORMAT (compact|pretty|json) — default compact
    // keeps --health / --list-tools JSON output easy to parse.
    makakoo_core::telemetry::init_stderr("info");

    let args = Args::parse();

    // Build the tool context. At T12 this is best-effort: we wire the
    // subsystems that open cleanly from env defaults and leave the rest
    // as `None`. T13/T14/T15 handlers extend this as they add tools that
    // need specific subsystems.
    let ctx = Arc::new(build_context().await?);

    // Build the registry and let the handlers module wire every tool.
    let mut registry = ToolRegistry::new();
    handlers::register_all(&mut registry, &ctx);
    let registry = Arc::new(registry);

    if args.health {
        println!("{{\"ok\":true,\"tools\":{}}}", registry.count());
        return Ok(());
    }

    if args.list_tools {
        let tools = registry.list();
        println!("{}", serde_json::to_string_pretty(&tools)?);
        return Ok(());
    }

    if let Some(http_arg) = args.http.as_deref() {
        return run_http(http_arg, args.bind, args.trust_file, args.signing_key, registry, ctx)
            .await;
    }

    let server = server::McpServer::new(registry, ctx);
    server.run_stdio().await?;
    Ok(())
}

/// v0.6 Phase B — start the HTTP serve mode. Loads (or generates) the
/// local signing keypair, loads the trust file, then hands everything to
/// `http_server::serve`.
async fn run_http(
    http_arg: &str,
    bind_ip: std::net::IpAddr,
    trust_file_arg: Option<std::path::PathBuf>,
    signing_key_arg: Option<std::path::PathBuf>,
    registry: Arc<dispatch::ToolRegistry>,
    ctx: Arc<dispatch::ToolContext>,
) -> Result<()> {
    use makakoo_core::adapter::peer;

    // Parse the bind spec. Accept "host:port" or bare ":port" — bare
    // port uses --bind as the host.
    let bind_addr: std::net::SocketAddr = match http_arg.parse() {
        Ok(addr) => addr,
        Err(_) => {
            // Try bare port.
            let port: u16 = http_arg
                .trim_start_matches(':')
                .parse()
                .map_err(|_| anyhow::anyhow!("--http must be ADDR:PORT or :PORT"))?;
            std::net::SocketAddr::new(bind_ip, port)
        }
    };

    let home = makakoo_core::platform::makakoo_home();
    let signing_key_path = signing_key_arg.unwrap_or_else(|| peer::default_signing_key_path(&home));
    let signing_pub_path = peer::default_signing_pub_path(&home);
    let trust_path = trust_file_arg.unwrap_or_else(|| peer::default_trust_file(&home));

    let (_signing, verifying, was_generated) =
        peer::load_or_create_signing_key(&signing_key_path, &signing_pub_path)
            .map_err(|e| anyhow::anyhow!("signing key: {e}"))?;

    if was_generated {
        use base64::Engine;
        let pubkey_b64 = base64::engine::general_purpose::STANDARD.encode(verifying.to_bytes());
        eprintln!(
            "makakoo-mcp: generated Ed25519 signing key at {}\n\
             pubkey (share with peers via `makakoo adapter trust add <name> <pubkey>`):\n\
             {}",
            signing_key_path.display(),
            pubkey_b64
        );
    }

    let trust = peer::load_trust_file(&trust_path)
        .map_err(|e| anyhow::anyhow!("trust file {}: {e}", trust_path.display()))?;

    if trust.is_empty() {
        tracing::warn!(
            path = %trust_path.display(),
            "trust file is empty or missing — all signed requests will be rejected until peers are added via `makakoo adapter trust add`"
        );
    }

    let state = Arc::new(http_server::HttpState::new(
        registry,
        ctx,
        trust,
        trust_path,
    ));
    http_server::serve(bind_addr, state).await
}

/// Build a best-effort [`ToolContext`]. T13 extends this from the T12
/// minimal skeleton to wire every subsystem the Tier-A handlers touch:
///
///   - [`LlmClient`] / [`EmbeddingClient`]: always constructed (env-driven).
///   - [`SuperbrainStore`]: opens `$MAKAKOO_HOME/data/superbrain.db`.
///   - [`GraphStore`] + [`MemoryStack`]: share the store's connection.
///   - [`ChatStore`]: opens `$MAKAKOO_HOME/data/chat.db`.
///   - [`CostTracker`]: shares the superbrain connection (costs table).
///   - [`MascotRegistry`] + [`BuddyTracker`]: JSON files under `data/`.
///   - [`AgentScaffold`]: rooted at `$MAKAKOO_HOME/agents`.
///
/// Every subsystem construction is wrapped in a best-effort block —
/// a failure logs a warning and leaves that slot `None` so `--list-tools`
/// and `--health` stay usable even on a fresh clone with no Brain yet.
async fn build_context() -> Result<ToolContext> {
    let home = makakoo_core::platform::makakoo_home();
    let data = home.join("data");

    let llm = Arc::new(makakoo_core::llm::LlmClient::new());
    let emb = Arc::new(makakoo_core::embeddings::EmbeddingClient::new());

    let mut ctx = ToolContext::empty(home.clone())
        .with_llm(llm)
        .with_embeddings(emb);

    // Superbrain store (+ graph, memory stack, cost tracker sharing its conn).
    match makakoo_core::superbrain::store::SuperbrainStore::open(&data.join("superbrain.db")) {
        Ok(store) => {
            let conn = store.conn_arc();
            let store_arc = Arc::new(store);
            let graph = Arc::new(makakoo_core::superbrain::graph::GraphStore::new(
                Arc::clone(&conn),
            ));
            let memory = Arc::new(makakoo_core::superbrain::memory_stack::MemoryStack::new(
                Arc::clone(&conn),
                Arc::clone(&graph),
            ));
            let costs = Arc::new(makakoo_core::telemetry::CostTracker::new(Arc::clone(&conn)));
            // Sprint-010 Phase A: wire RecallTracker so MCP brain-search
            // handlers can write to `recall_log`. Shares the same sqlite
            // connection as the superbrain store — `track_batch` runs in
            // its own transaction so writer contention is serialised
            // without a second DB handle.
            let recall = Arc::new(makakoo_core::superbrain::recall::RecallTracker::new(
                Arc::clone(&conn),
            ));
            ctx = ctx
                .with_store(store_arc)
                .with_graph(graph)
                .with_memory(memory)
                .with_costs(costs)
                .with_recall(recall);

            // Outbound queue shares the superbrain connection — the
            // `outbound_drafts` table is part of the same migration set
            // `SuperbrainStore::open` just ran. Tier-B `outbound_draft`
            // depends on this being wired.
            match makakoo_core::outbound::OutboundQueue::open(Arc::clone(&conn)) {
                Ok(q) => {
                    ctx = ctx.with_outbound(Arc::new(q));
                }
                Err(e) => {
                    tracing::warn!(%e, "outbound queue unavailable; outbound_draft will error");
                }
            }
        }
        Err(e) => {
            tracing::warn!(%e, "superbrain store unavailable; brain_* tools will error");
        }
    }

    // Chat store.
    match makakoo_core::chat::ChatStore::open(&data.join("chat.db")) {
        Ok(chat) => {
            ctx = ctx.with_chat(Arc::new(chat));
        }
        Err(e) => {
            tracing::warn!(%e, "chat store unavailable; chat_* tools will error");
        }
    }

    // Nursery + buddy. Nursery seeds canonical mascots on first load.
    match makakoo_core::nursery::MascotRegistry::load(&data.join("nursery.json")) {
        Ok(registry) => {
            let registry_arc = Arc::new(registry);
            ctx = ctx.with_nursery(Arc::clone(&registry_arc));
            match makakoo_core::nursery::BuddyTracker::load(
                Arc::clone(&registry_arc),
                &data.join("buddy.json"),
            ) {
                Ok(buddy) => {
                    ctx = ctx.with_buddy(Arc::new(buddy));
                }
                Err(e) => {
                    tracing::warn!(%e, "buddy tracker unavailable");
                }
            }
        }
        Err(e) => {
            tracing::warn!(%e, "mascot registry unavailable");
        }
    }

    // Agent scaffold — just a path root.
    let scaffold = makakoo_core::agents::AgentScaffold::new(home.join("agents"));
    ctx = ctx.with_agents(Arc::new(scaffold));

    // T15 — swarm subsystem. Needs a SQLite connection for the artifact
    // store (the `swarm_artifacts` table lives in the unified schema)
    // plus an LLM client + event bus for the gateway runner. All three
    // are best-effort: a missing subsystem leaves `swarm_state = None`
    // and the Tier-C handlers return a structured "not wired" error.
    match build_swarm_state(&data).await {
        Ok(state) => {
            ctx = ctx.with_swarm_state(state);
        }
        Err(e) => {
            tracing::warn!(%e, "swarm subsystem unavailable; harvey_swarm_* tools will error");
        }
    }

    Ok(ctx)
}

/// Wire the swarm subsystem (artifact store + coordinator + gateway +
/// event bus) using the standard makakoo data dir paths. Separated from
/// `build_context` so its failure modes stay contained and the main
/// build still returns a usable context without swarm support.
async fn build_swarm_state(
    data: &std::path::Path,
) -> Result<Arc<makakoo_core::swarm::SwarmState>> {
    use std::sync::Mutex;

    let map = |e: makakoo_core::MakakooError| anyhow::anyhow!("{e}");
    let db_path = data.join("swarm.db");
    let conn = makakoo_core::db::open_db(&db_path).map_err(map)?;
    makakoo_core::db::run_migrations(&conn).map_err(map)?;
    let conn_arc = Arc::new(Mutex::new(conn));
    let artifacts = Arc::new(
        makakoo_core::swarm::ArtifactStore::open(Arc::clone(&conn_arc)).map_err(map)?,
    );
    let coordinator = Arc::new(makakoo_core::swarm::AgentCoordinator::new());
    let llm = Arc::new(makakoo_core::llm::LlmClient::new());
    let bus = makakoo_core::event_bus::PersistentEventBus::open(&data.join("events.db"))
        .map_err(map)?;
    let gateway = Arc::new(makakoo_core::swarm::SwarmGateway::new(
        Arc::clone(&coordinator),
        Arc::clone(&artifacts),
        llm,
        bus,
    ));
    Ok(Arc::new(makakoo_core::swarm::SwarmState {
        gateway,
        coordinator,
        artifacts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint-010 regression guard: whenever the superbrain store is
    /// reachable, `build_context` MUST also wire a `RecallTracker`.
    /// Without this, MCP brain-search handlers silently skip the
    /// `track_batch` call (ctx.recall.is_none()) and no `recall_log`
    /// rows get written — which was the original bug Phase A fixed at
    /// the handler layer but left un-wired at the context layer.
    #[tokio::test]
    async fn build_context_wires_recall_when_store_is_reachable() {
        let dir = tempfile::tempdir().unwrap();
        // Point MAKAKOO_HOME at the tempdir so build_context opens a
        // fresh store instead of the real one.
        let prev = std::env::var("MAKAKOO_HOME").ok();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let ctx = build_context().await.unwrap();
        match prev {
            Some(v) => std::env::set_var("MAKAKOO_HOME", v),
            None => std::env::remove_var("MAKAKOO_HOME"),
        }
        assert!(ctx.store.is_some(), "store should open under fresh MAKAKOO_HOME");
        assert!(
            ctx.recall.is_some(),
            "recall tracker must be wired whenever store is — sprint-010 phase-a"
        );
    }
}
