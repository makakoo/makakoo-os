//! SANCHO built-in handlers — Rust ports of `core/sancho/handlers.py`.
//!
//! Each handler is a thin wrapper around a Makakoo subsystem that writes
//! its summary line into today's Brain journal via [`append_journal_line`]
//! and returns a [`HandlerReport`]. Handlers that call the LLM route
//! through the [`LlmCall`] trait so unit tests can inject a
//! [`FakeLlmCall`] without touching a real switchAILocal.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Local;
use regex::Regex;
use serde_json::json;

use crate::error::Result;
use crate::llm::{ChatMessage, LlmClient};
use crate::sancho::registry::{HandlerReport, SanchoContext, SanchoHandler};
use crate::superbrain::promoter::MemoryPromoter;

/// LLM indirection used by handlers. `LlmClient` implements this for
/// production; [`FakeLlmCall`] is used in unit tests.
#[async_trait]
pub trait LlmCall: Send + Sync {
    async fn chat(&self, model: &str, prompt: &str) -> Result<String>;
}

#[async_trait]
impl LlmCall for LlmClient {
    async fn chat(&self, model: &str, prompt: &str) -> Result<String> {
        LlmClient::chat(self, model, vec![ChatMessage::user(prompt)]).await
    }
}

/// Cheap in-process LLM stub for tests.
pub struct FakeLlmCall {
    pub reply: String,
}

impl FakeLlmCall {
    pub fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
        }
    }
}

#[async_trait]
impl LlmCall for FakeLlmCall {
    async fn chat(&self, _model: &str, _prompt: &str) -> Result<String> {
        Ok(self.reply.clone())
    }
}

// ─────────────────────────────────────────────────────────────────────
//  Shared helpers
// ─────────────────────────────────────────────────────────────────────

const DREAM_MODEL: &str = "auto";

/// Truncate `s` to at most `max` chars, appending an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    let trimmed: String = s.chars().filter(|c| *c != '\n').collect();
    if trimmed.chars().count() <= max {
        return trimmed;
    }
    let head: String = trimmed.chars().take(max).collect();
    format!("{head}…")
}

/// Append a single outliner line to today's Brain journal.
pub fn append_journal_line(home: &Path, line: &str) -> Result<PathBuf> {
    let today = Local::now().format("%Y_%m_%d").to_string();
    let journal_dir = home.join("data").join("Brain").join("journals");
    fs::create_dir_all(&journal_dir)?;
    let journal_path = journal_dir.join(format!("{today}.md"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)?;
    let needs_nl = journal_path
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if needs_nl {
        let current = fs::read(&journal_path).unwrap_or_default();
        if !current.ends_with(b"\n") {
            writeln!(file)?;
        }
    }
    let entry = if line.starts_with("- ") {
        line.to_string()
    } else {
        format!("- {line}")
    };
    writeln!(file, "{entry}")?;
    Ok(journal_path)
}

fn publish_report(ctx: &SanchoContext, report: &HandlerReport) {
    let payload = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
    let _ = ctx.bus.publish(
        "sancho.handler.completed",
        "sancho",
        json!({
            "handler": report.handler,
            "ok": report.ok,
            "message": report.message,
            "duration_sec": report.duration.as_secs_f64(),
            "report": payload,
        }),
    );
}

// ─────────────────────────────────────────────────────────────────────
//  1. DreamHandler
// ─────────────────────────────────────────────────────────────────────

/// Run a dream-style consolidation pass over the most recent documents.
pub struct DreamHandler {
    pub limit: usize,
    pub llm: Arc<dyn LlmCall>,
    pub model: String,
}

impl DreamHandler {
    pub fn new(llm: Arc<dyn LlmCall>) -> Self {
        Self {
            limit: 20,
            llm,
            model: DREAM_MODEL.to_string(),
        }
    }
}

#[async_trait]
impl SanchoHandler for DreamHandler {
    fn name(&self) -> &str {
        "dream"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let recent = ctx.store.recent(self.limit, None)?;
        let joined: String = recent
            .iter()
            .map(|h| format!("- {}: {}", h.doc_id, truncate(&h.content, 160)))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "You are Harvey's dream consolidator. Summarize the following \
             recent Brain docs into ONE tight paragraph (≤4 sentences). No \
             preamble, no headers.\n\n{joined}"
        );
        let reply = self.llm.chat(&self.model, &prompt).await?;
        let summary = reply.trim().to_string();
        let line = format!(
            "- [[SANCHO]] dream consolidation ({} docs): {}",
            recent.len(),
            summary
        );
        append_journal_line(&ctx.home, &line)?;
        let report = HandlerReport::ok(
            "dream",
            format!("consolidated {} docs", recent.len()),
            start.elapsed(),
        );
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  2. WikiLintHandler
// ─────────────────────────────────────────────────────────────────────

/// Scan `data/Brain/pages/**/*.md` and emit a lint report.
pub struct WikiLintHandler;

impl WikiLintHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WikiLintHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn walk_md(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_md(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(p);
        }
    }
}

#[async_trait]
impl SanchoHandler for WikiLintHandler {
    fn name(&self) -> &str {
        "wiki_lint"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let pages_dir = ctx.home.join("data").join("Brain").join("pages");
        let mut pages = Vec::new();
        walk_md(&pages_dir, &mut pages);

        let wikilink_re = Regex::new(r"\[\[([^\]]+)\]\]").expect("regex");
        let mut page_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in &pages {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                page_names.insert(stem.to_string());
            }
        }

        let mut empty = 0usize;
        let mut missing_targets = 0usize;
        let mut total_links = 0usize;
        for p in &pages {
            let Ok(content) = fs::read_to_string(p) else {
                continue;
            };
            let non_blank = content.lines().filter(|l| !l.trim().is_empty()).count();
            if non_blank <= 1 {
                empty += 1;
            }
            for cap in wikilink_re.captures_iter(&content) {
                total_links += 1;
                let target = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                if !page_names.contains(target) {
                    missing_targets += 1;
                }
            }
        }

        let msg = format!(
            "{} pages, {} empty, {}/{} missing wikilink targets",
            pages.len(),
            empty,
            missing_targets,
            total_links
        );
        append_journal_line(&ctx.home, &format!("- [[SANCHO]] wiki_lint: {msg}"))?;
        let report = HandlerReport::ok("wiki_lint", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  3. IndexRebuildHandler
// ─────────────────────────────────────────────────────────────────────

/// Refresh + journal the superbrain store stats.
pub struct IndexRebuildHandler;

impl IndexRebuildHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IndexRebuildHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for IndexRebuildHandler {
    fn name(&self) -> &str {
        "index_rebuild"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let stats = ctx.store.stats()?;
        let msg = format!("{stats:?}");
        append_journal_line(
            &ctx.home,
            &format!("- [[SANCHO]] index_rebuild: stats refreshed — {msg}"),
        )?;
        let report = HandlerReport::ok("index_rebuild", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  4. DailyBriefingHandler
// ─────────────────────────────────────────────────────────────────────

/// Generate a short morning briefing from recent Brain docs using the LLM.
pub struct DailyBriefingHandler {
    pub llm: Arc<dyn LlmCall>,
    pub model: String,
    pub limit: usize,
}

impl DailyBriefingHandler {
    pub fn new(llm: Arc<dyn LlmCall>) -> Self {
        Self {
            llm,
            model: DREAM_MODEL.to_string(),
            limit: 15,
        }
    }
}

#[async_trait]
impl SanchoHandler for DailyBriefingHandler {
    fn name(&self) -> &str {
        "daily_briefing"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let recent = ctx.store.recent(self.limit, None)?;
        let joined: String = recent
            .iter()
            .map(|h| format!("- {}: {}", h.doc_id, truncate(&h.content, 160)))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "You are Harvey's morning briefer. Write a 3-bullet briefing \
             covering today's top priorities based on the recent Brain \
             activity below. Bullets must start with `- `.\n\n{joined}"
        );
        let reply = self.llm.chat(&self.model, &prompt).await?;
        let body = reply.trim();
        append_journal_line(
            &ctx.home,
            &format!(
                "- [[SANCHO]] daily_briefing:\n  {}",
                body.replace('\n', "\n  ")
            ),
        )?;
        let report = HandlerReport::ok(
            "daily_briefing",
            format!("briefing over {} docs", recent.len()),
            start.elapsed(),
        );
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  5. MemoryConsolidationHandler
// ─────────────────────────────────────────────────────────────────────

/// Rank promotion candidates without mutating long-term storage.
pub struct MemoryConsolidationHandler;

impl MemoryConsolidationHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemoryConsolidationHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for MemoryConsolidationHandler {
    fn name(&self) -> &str {
        "memory_consolidation"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let promoter = MemoryPromoter::new(ctx.store.conn_arc());
        let ranked = promoter.rank_candidates()?;
        let msg = format!("{} candidates ranked", ranked.len());
        append_journal_line(
            &ctx.home,
            &format!("- [[SANCHO]] memory_consolidation: {msg}"),
        )?;
        let report = HandlerReport::ok("memory_consolidation", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  6. MemoryPromotionHandler
// ─────────────────────────────────────────────────────────────────────

/// Promote the top-K ranked candidates to long-term memory.
pub struct MemoryPromotionHandler {
    pub top_k: usize,
    pub threshold: f32,
}

impl MemoryPromotionHandler {
    pub fn new() -> Self {
        Self {
            top_k: 5,
            threshold: 0.5,
        }
    }
}

impl Default for MemoryPromotionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for MemoryPromotionHandler {
    fn name(&self) -> &str {
        "memory_promotion"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let promoter = MemoryPromoter::new(ctx.store.conn_arc());
        let promoted = promoter.promote_candidates(self.threshold, self.top_k)?;
        let msg = format!("{} promotions", promoted.len());
        append_journal_line(
            &ctx.home,
            &format!("- [[SANCHO]] memory_promotion: {msg}"),
        )?;
        let report = HandlerReport::ok("memory_promotion", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  7. SuperbrainSyncEmbedHandler
// ─────────────────────────────────────────────────────────────────────

/// Find docs missing vectors and embed them via the injected
/// [`crate::embeddings::EmbeddingClient`].
pub struct SuperbrainSyncEmbedHandler {
    pub max_docs: usize,
}

impl SuperbrainSyncEmbedHandler {
    pub fn new() -> Self {
        Self { max_docs: 32 }
    }
}

impl Default for SuperbrainSyncEmbedHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for SuperbrainSyncEmbedHandler {
    fn name(&self) -> &str {
        "superbrain_sync_embed"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let candidates = ctx.store.docs_missing_vectors(self.max_docs)?;
        let mut embedded = 0usize;
        for (doc_id, text) in candidates {
            match ctx.emb.embed(&text).await {
                Ok(vec) => {
                    if ctx.store.store_vector(&doc_id, &vec).is_ok() {
                        embedded += 1;
                    }
                }
                Err(_) => continue,
            }
        }
        let msg = format!("{embedded} vectors embedded");
        append_journal_line(
            &ctx.home,
            &format!("- [[SANCHO]] superbrain_sync_embed: {msg}"),
        )?;
        let report = HandlerReport::ok("superbrain_sync_embed", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  8. DynamicChecklistHandler
// ─────────────────────────────────────────────────────────────────────

/// Generate a short next-steps checklist from recent Brain activity.
pub struct DynamicChecklistHandler {
    pub llm: Arc<dyn LlmCall>,
    pub model: String,
    pub limit: usize,
}

impl DynamicChecklistHandler {
    pub fn new(llm: Arc<dyn LlmCall>) -> Self {
        Self {
            llm,
            model: DREAM_MODEL.to_string(),
            limit: 20,
        }
    }
}

#[async_trait]
impl SanchoHandler for DynamicChecklistHandler {
    fn name(&self) -> &str {
        "dynamic_checklist"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let recent = ctx.store.recent(self.limit, None)?;
        let joined: String = recent
            .iter()
            .map(|h| format!("- {}: {}", h.doc_id, truncate(&h.content, 160)))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "Read the recent Brain activity below and emit a short \
             checklist (≤5 items) of concrete next steps. Each line starts \
             with `- [ ] `. If nothing is actionable, respond with the \
             single token NOOP.\n\n{joined}"
        );
        let reply = self.llm.chat(&self.model, &prompt).await?;
        let body = reply.trim();
        let suppressed = body.is_empty() || body == "NOOP";
        if !suppressed {
            append_journal_line(
                &ctx.home,
                &format!(
                    "- [[SANCHO]] dynamic_checklist:\n  {}",
                    body.replace('\n', "\n  ")
                ),
            )?;
        }
        let report = HandlerReport::ok(
            "dynamic_checklist",
            if suppressed {
                "suppressed (NOOP)".to_string()
            } else {
                format!("{} bytes of checklist", body.len())
            },
            start.elapsed(),
        );
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  9. SubprocessHandler — generic wrapper for legacy Python watchdogs
// ─────────────────────────────────────────────────────────────────────

/// Runs an arbitrary shell command on each tick and records its exit status
/// in today's Brain journal. Used to schedule legacy Python scripts from
/// Rust SANCHO instead of crontab — bridges the Python→Rust migration for
/// tasks that haven't been ported to native Rust yet.
///
/// Captures up to 200 chars of stdout for the journal line; truncates longer
/// output. Failures (non-zero exit) are logged but don't crash the engine.
pub struct SubprocessHandler {
    task_name: String,
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
}

impl SubprocessHandler {
    pub fn new(
        task_name: impl Into<String>,
        program: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            task_name: task_name.into(),
            program: program.into(),
            args,
            cwd: None,
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

#[async_trait]
impl SanchoHandler for SubprocessHandler {
    fn name(&self) -> &str {
        &self.task_name
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        let start = Instant::now();
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args);
        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        } else {
            cmd.current_dir(&ctx.home);
        }
        // Give the subprocess the canonical env so Python scripts resolve
        // paths correctly even if the launchd env doesn't carry them.
        cmd.env("MAKAKOO_HOME", &ctx.home);
        cmd.env("HARVEY_HOME", &ctx.home);
        cmd.env(
            "PYTHONPATH",
            ctx.home
                .join("harvey-os")
                .to_string_lossy()
                .into_owned(),
        );

        let output = cmd.output().await;
        let elapsed = start.elapsed();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let summary = truncate(&stdout, 200);
                let line = if summary.is_empty() {
                    format!("- [[SANCHO]] {}: ok ({}ms)", self.task_name, elapsed.as_millis())
                } else {
                    format!(
                        "- [[SANCHO]] {}: ok — {} ({}ms)",
                        self.task_name,
                        summary,
                        elapsed.as_millis()
                    )
                };
                append_journal_line(&ctx.home, &line)?;
                let report = HandlerReport::ok(&self.task_name, summary, elapsed);
                publish_report(ctx, &report);
                Ok(report)
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let summary = truncate(&stderr, 200);
                let code = out.status.code().unwrap_or(-1);
                let line = format!(
                    "- [[SANCHO]] {}: FAILED (exit {}) — {}",
                    self.task_name, code, summary
                );
                let _ = append_journal_line(&ctx.home, &line);
                let report = HandlerReport::failed(
                    &self.task_name,
                    format!("exit {code}: {summary}"),
                    elapsed,
                );
                publish_report(ctx, &report);
                Ok(report)
            }
            Err(e) => {
                let line = format!(
                    "- [[SANCHO]] {}: SPAWN ERROR — {}",
                    self.task_name, e
                );
                let _ = append_journal_line(&ctx.home, &line);
                let report = HandlerReport::failed(
                    &self.task_name,
                    format!("spawn error: {e}"),
                    elapsed,
                );
                publish_report(ctx, &report);
                Ok(report)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
//  10. SwarmDispatchHandler — drains $MAKAKOO_HOME/state/swarm/queue.jsonl
// ─────────────────────────────────────────────────────────────────────

/// Drain pending swarm dispatch requests on every tick.
///
/// v0.2 Phase D.4. Producers enqueue `TeamDispatchRequest` /
/// `DispatchRequest` via `swarm::dispatch_queue::{enqueue_team,
/// enqueue_agent}`. On each tick the handler calls the swarm gateway
/// for every pending entry, writes a receipt, and journals one line
/// per successful dispatch.
///
/// The gateway is looked up via [`crate::swarm::SwarmGateway::global`].
/// If the global gateway isn't installed (tests, boots without swarm)
/// the handler reports ok-but-noop — it doesn't fail a tick.
pub struct SwarmDispatchHandler {
    /// Upper bound per tick so a pathological queue can't starve the
    /// rest of SANCHO. Remainder drains on the next tick.
    pub max_per_tick: usize,
}

impl SwarmDispatchHandler {
    pub fn new() -> Self {
        Self { max_per_tick: 16 }
    }
    pub fn with_max_per_tick(mut self, n: usize) -> Self {
        self.max_per_tick = n;
        self
    }
}

impl Default for SwarmDispatchHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for SwarmDispatchHandler {
    fn name(&self) -> &str {
        "swarm_dispatch"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        use crate::swarm::dispatch_queue::{load_receipts, load_queue, write_receipt, Receipt};
        use crate::swarm::gateway::SwarmGateway;
        use crate::swarm::QueueEntry;
        use std::collections::HashSet;

        let start = Instant::now();

        let queue = match load_queue(&ctx.home) {
            Ok(q) => q,
            Err(e) => {
                return Ok(HandlerReport::failed(
                    "swarm_dispatch",
                    format!("queue read failed: {e}"),
                    start.elapsed(),
                ));
            }
        };
        if queue.is_empty() {
            return Ok(HandlerReport::ok("swarm_dispatch", "queue empty", start.elapsed()));
        }
        let receipts = load_receipts(&ctx.home).unwrap_or_default();
        let done: HashSet<String> = receipts.into_iter().map(|r| r.id).collect();

        let gateway = match SwarmGateway::global() {
            Some(g) => g,
            None => {
                return Ok(HandlerReport::ok(
                    "swarm_dispatch",
                    format!("gateway not installed; {} pending", queue.len() - done.len()),
                    start.elapsed(),
                ));
            }
        };

        let mut dispatched = 0usize;
        let mut failures = 0usize;

        for entry in queue.into_iter().filter(|e| !done.contains(e.id())) {
            if dispatched + failures >= self.max_per_tick {
                break;
            }
            let id = entry.id().to_string();
            let outcome = match entry {
                QueueEntry::Team { req, .. } => {
                    let team_name = req.team.clone();
                    match crate::swarm::TeamComposition::by_name(&team_name, req.parallelism) {
                        Some(roster) => match gateway.dispatch_team(&roster, req).await {
                            Ok(resp) => Ok(resp.run_id),
                            Err(e) => Err(format!("team {team_name}: {e}")),
                        },
                        None => Err(format!("unknown team {team_name}")),
                    }
                }
                QueueEntry::Agent { req, .. } => match gateway.dispatch(req).await {
                    Ok(resp) => Ok(resp.run_id),
                    Err(e) => Err(format!("{e}")),
                },
            };

            match outcome {
                Ok(run_id) => {
                    let r = Receipt {
                        id: id.clone(),
                        dispatched_at: chrono::Utc::now(),
                        run_id: run_id.clone(),
                    };
                    if let Err(e) = write_receipt(&ctx.home, &r) {
                        tracing::warn!(queue_id = %id, error = %e, "receipt write failed");
                        failures += 1;
                        continue;
                    }
                    let _ = append_journal_line(
                        &ctx.home,
                        &format!(
                            "- [[SANCHO]] [[swarm_dispatch]] queued `{}` → run `{}`",
                            id, run_id
                        ),
                    );
                    dispatched += 1;
                }
                Err(msg) => {
                    tracing::warn!(queue_id = %id, error = %msg, "dispatch failed — will retry");
                    failures += 1;
                }
            }
        }

        let report = HandlerReport::ok(
            "swarm_dispatch",
            format!("dispatched={dispatched} failures={failures}"),
            start.elapsed(),
        );
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  10. PermsPurgeHandler (v0.3 user grants)
// ─────────────────────────────────────────────────────────────────────

/// Drop expired user grants from `$MAKAKOO_HOME/config/user_grants.json`
/// and emit one `perms/revoke` audit entry per removed grant with
/// `correlation_id="reason:expired"`.
///
/// The CLI `makakoo perms purge` (Phase D) shares the same `UserGrants`
/// backend — the only divergence is this handler's plugin attribution
/// (`sancho-native`) and its audit-log correlation marker.
///
/// No-op when the file is missing or already clean. Idempotent: two
/// ticks in a row emit audit entries only for grants that were still
/// active on the first tick.
pub struct PermsPurgeHandler;

impl PermsPurgeHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PermsPurgeHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SanchoHandler for PermsPurgeHandler {
    fn name(&self) -> &str {
        "perms_purge_tick"
    }

    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport> {
        use crate::capability::audit::{AuditEntry, AuditLog, AuditResult};
        use crate::capability::user_grants::UserGrants;
        use chrono::Utc;

        let start = Instant::now();
        let now = Utc::now();

        let mut grants = UserGrants::load(&ctx.home);
        let removed = grants.purge_expired(now);
        if removed.is_empty() {
            let report = HandlerReport::ok(
                "perms_purge_tick",
                "0 expired",
                start.elapsed(),
            );
            publish_report(ctx, &report);
            return Ok(report);
        }

        if let Err(e) = grants.save() {
            return Ok(HandlerReport::failed(
                "perms_purge_tick",
                format!("save failed: {e}"),
                start.elapsed(),
            ));
        }

        let audit = match AuditLog::open_default(&ctx.home) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("perms_purge_tick: audit log open failed: {e}");
                None
            }
        };
        if let Some(audit) = audit.as_ref() {
            for g in &removed {
                let entry = AuditEntry {
                    ts: now,
                    plugin: "sancho-native".to_string(),
                    plugin_version: env!("CARGO_PKG_VERSION").to_string(),
                    verb: "perms/revoke".to_string(),
                    scope_requested: g.scope.clone(),
                    scope_granted: Some(g.id.clone()),
                    result: AuditResult::Allowed,
                    duration_ms: None,
                    bytes_in: None,
                    bytes_out: None,
                    correlation_id: Some("reason:expired".to_string()),
                };
                if let Err(e) = audit.append(&entry) {
                    tracing::warn!("perms_purge_tick: audit append failed for {}: {e}", g.id);
                }
            }
        }

        let msg = format!("{} expired", removed.len());
        let _ = append_journal_line(
            &ctx.home,
            &format!("- [[SANCHO]] perms_purge_tick: {msg}"),
        );
        let report = HandlerReport::ok("perms_purge_tick", msg, start.elapsed());
        publish_report(ctx, &report);
        Ok(report)
    }
}

// ─────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::EmbeddingClient;
    use crate::event_bus::PersistentEventBus;
    use crate::llm::LlmClient;
    use crate::superbrain::store::SuperbrainStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn ctx_with_store(dir: &TempDir) -> SanchoContext {
        let store_path = dir.path().join("brain.db");
        let bus_path = dir.path().join("events.db");
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("data").join("Brain").join("journals")).unwrap();
        fs::create_dir_all(home.join("data").join("Brain").join("pages")).unwrap();

        let store = Arc::new(SuperbrainStore::open(&store_path).unwrap());
        let bus = PersistentEventBus::open(&bus_path).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        SanchoContext::new(store, bus, llm, emb, home)
    }

    #[tokio::test]
    async fn dream_handler_writes_journal() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        ctx.store
            .write_document(
                "doc-1",
                "Body snippet 1 for dream consolidation",
                "page",
                serde_json::json!([]),
            )
            .unwrap();

        let llm: Arc<dyn LlmCall> = Arc::new(FakeLlmCall::new("consolidated summary"));
        let h = DreamHandler::new(llm);
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert_eq!(report.handler, "dream");

        let journal_glob = ctx.home.join("data").join("Brain").join("journals");
        let any_journal = fs::read_dir(&journal_glob)
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"));
        assert!(any_journal, "expected today's journal to exist");
    }

    #[tokio::test]
    async fn wiki_lint_handler_counts_pages() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let pages = ctx.home.join("data").join("Brain").join("pages");
        fs::write(
            pages.join("Alpha.md"),
            "- [[Beta]] links here\n- body content\n",
        )
        .unwrap();
        fs::write(pages.join("Beta.md"), "- real page\n").unwrap();
        fs::write(pages.join("Empty.md"), "\n").unwrap();

        let h = WikiLintHandler::new();
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("3 pages"));
    }

    #[tokio::test]
    async fn index_rebuild_reports_stats() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let h = IndexRebuildHandler::new();
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert_eq!(report.handler, "index_rebuild");
    }

    #[tokio::test]
    async fn daily_briefing_writes_journal_via_fake_llm() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        ctx.store
            .write_document(
                "journals/d1.md",
                "Did stuff yesterday",
                "journal",
                serde_json::json!([]),
            )
            .unwrap();
        let llm: Arc<dyn LlmCall> = Arc::new(FakeLlmCall::new("- [ ] ship T8\n- [ ] sleep"));
        let h = DailyBriefingHandler::new(llm);
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("briefing"));
    }

    #[tokio::test]
    async fn memory_consolidation_runs_even_on_empty_store() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let h = MemoryConsolidationHandler::new();
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("candidates"));
    }

    #[tokio::test]
    async fn memory_promotion_runs_even_on_empty_store() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let h = MemoryPromotionHandler::new();
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("promotions"));
    }

    #[tokio::test]
    async fn dynamic_checklist_suppresses_on_noop() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let llm: Arc<dyn LlmCall> = Arc::new(FakeLlmCall::new("NOOP"));
        let h = DynamicChecklistHandler::new(llm);
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("suppressed"));
    }

    #[tokio::test]
    async fn dynamic_checklist_writes_on_actionable() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let llm: Arc<dyn LlmCall> =
            Arc::new(FakeLlmCall::new("- [ ] do thing\n- [ ] do other thing"));
        let h = DynamicChecklistHandler::new(llm);
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(!report.message.contains("suppressed"));
    }

    #[test]
    fn append_journal_line_creates_dir_and_file() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().to_path_buf();
        let path = append_journal_line(&home, "hello").unwrap();
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("- hello"));
    }

    #[test]
    fn append_journal_line_preserves_prefix() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().to_path_buf();
        let _ = append_journal_line(&home, "- already prefixed").unwrap();
        // Second call should not double-prefix.
        let path = append_journal_line(&home, "- already prefixed").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("- - already"));
    }

    // D.4 — SwarmDispatchHandler. We don't have a global SwarmGateway in
    // unit tests (they'd need a real event bus + coordinator), so the
    // handler short-circuits with a benign "gateway not installed" report.
    // Pre-handler: queue has 2 entries, 1 already receipted. Post-handler:
    // state is untouched (gateway missing → no side-effects).
    #[tokio::test]
    async fn swarm_dispatch_handler_noops_without_gateway() {
        use crate::swarm::{enqueue_team, Receipt};
        use crate::swarm::dispatch_queue::write_receipt;
        use crate::swarm::TeamDispatchRequest;

        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);

        let id1 = enqueue_team(
            &ctx.home,
            TeamDispatchRequest {
                team: "research_team".into(),
                prompt: "investigate X".into(),
                parallelism: None,
                model: None,
            },
        )
        .unwrap();
        let _id2 = enqueue_team(
            &ctx.home,
            TeamDispatchRequest {
                team: "archive_team".into(),
                prompt: "file Y".into(),
                parallelism: None,
                model: None,
            },
        )
        .unwrap();
        write_receipt(
            &ctx.home,
            &Receipt {
                id: id1.clone(),
                dispatched_at: chrono::Utc::now(),
                run_id: "already-run".into(),
            },
        )
        .unwrap();

        let handler = SwarmDispatchHandler::new();
        let report = handler.run(&ctx).await.unwrap();
        assert!(report.ok, "handler must report ok when gateway is absent");
        assert!(
            report.message.contains("gateway not installed")
                || report.message.contains("queue empty"),
            "unexpected message: {}",
            report.message,
        );
    }

    #[tokio::test]
    async fn swarm_dispatch_handler_ok_on_empty_queue() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let handler = SwarmDispatchHandler::new();
        let report = handler.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("queue empty"), "msg: {}", report.message);
    }

    // ─── PermsPurgeHandler (Phase F) ──────────────────────────────

    #[tokio::test]
    async fn perms_purge_removes_expired_keeps_active_and_emits_audit() {
        use crate::capability::user_grants::{default_path, UserGrant, UserGrants};
        use chrono::{Duration as ChronoDuration, Utc};

        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);
        let now = Utc::now();

        // Seed three grants: two expired, one active.
        let p = default_path(&ctx.home);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut u = UserGrants::empty_at(p);
        for (id, offset_minutes) in
            [("g_expired_a", -30i64), ("g_expired_b", -1), ("g_active_c", 60)]
        {
            u.add(UserGrant {
                id: id.into(),
                scope: format!("fs/write:/tmp/{id}/**"),
                created_at: now - ChronoDuration::minutes(120),
                expires_at: Some(now + ChronoDuration::minutes(offset_minutes)),
                label: "seed".into(),
                granted_by: "sebastian".into(),
                plugin: "cli".into(),
                origin_turn_id: "".into(),
                owner: "cli".into(),
            });
        }
        u.save().unwrap();

        let h = PermsPurgeHandler::new();
        let report = h.run(&ctx).await.unwrap();
        assert!(report.ok);
        assert!(report.message.contains("2 expired"), "msg: {}", report.message);

        // Disk shows only the active grant.
        let loaded = UserGrants::load(&ctx.home);
        assert_eq!(loaded.grants.len(), 1);
        assert_eq!(loaded.grants[0].id, "g_active_c");

        // Audit log has one perms/revoke entry per expired grant, each
        // carrying correlation_id="reason:expired" and plugin="sancho-native".
        let audit_path = ctx.home.join("logs").join("audit.jsonl");
        let contents = fs::read_to_string(&audit_path).unwrap();
        let lines: Vec<&str> = contents.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2, "expected 2 audit entries; got: {contents}");
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["plugin"], "sancho-native");
            assert_eq!(v["verb"], "perms/revoke");
            assert_eq!(v["result"], "allowed");
            assert_eq!(v["correlation_id"], "reason:expired");
            let gid = v["scope_granted"].as_str().unwrap();
            assert!(gid.starts_with("g_expired_"), "got scope_granted={gid}");
        }
    }

    #[tokio::test]
    async fn perms_purge_idempotent_no_op_on_clean_store() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx_with_store(&dir);

        // No grants file at all.
        let h = PermsPurgeHandler::new();
        let r1 = h.run(&ctx).await.unwrap();
        assert!(r1.ok);
        assert!(r1.message.contains("0 expired"), "msg: {}", r1.message);

        // Second tick back-to-back: still 0 expired, no audit rows.
        let r2 = h.run(&ctx).await.unwrap();
        assert!(r2.ok);
        assert!(r2.message.contains("0 expired"), "msg: {}", r2.message);

        let audit_path = ctx.home.join("logs").join("audit.jsonl");
        if audit_path.exists() {
            let contents = fs::read_to_string(&audit_path).unwrap();
            assert!(
                contents.trim().is_empty(),
                "audit log should be empty on no-op ticks; got: {contents}"
            );
        }
    }

    #[tokio::test]
    async fn perms_purge_tick_collision_plugin_is_skipped_by_walker() {
        // Belt-and-suspenders: if a rogue plugin ships a sancho task
        // named "perms_purge_tick", the walker MUST skip it so the
        // native handler stays authoritative.
        use crate::plugin::PluginRegistry;
        use crate::sancho::{default_registry, SanchoContext};

        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let plugin_dir = home.join("plugins").join("rogue-perms");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
[plugin]
name = "rogue-perms"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -m rogue"

[sancho]
tasks = [{ name = "perms_purge_tick", interval = "60s" }]
"#,
        )
        .unwrap();

        let plugins = PluginRegistry::load_default(home).unwrap();
        let store_path = home.join("b.db");
        let bus_path = home.join("bus.db");
        let store = Arc::new(SuperbrainStore::open(&store_path).unwrap());
        let bus = PersistentEventBus::open(&bus_path).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        let ctx = Arc::new(SanchoContext::new(store, bus, llm, emb, home.to_path_buf()));

        let reg = default_registry(ctx, &plugins);
        let names: Vec<&str> = reg.tasks().iter().map(|t| t.handler.name()).collect();
        let count = names.iter().filter(|n| **n == "perms_purge_tick").count();
        assert_eq!(
            count, 1,
            "walker must skip rogue plugin that shadows perms_purge_tick; names={names:?}"
        );
    }
}
