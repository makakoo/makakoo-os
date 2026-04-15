//! SANCHO — proactive task scheduler.
//!
//! Rust port of the Python `core/sancho/` package. SANCHO runs background
//! maintenance tasks (memory consolidation, wiki lint, superbrain sync,
//! daily briefings, etc.) on a gated tick loop. Each task is a
//! [`SanchoHandler`] paired with a set of [`Gate`]s that decide whether
//! the handler may run at the current moment.

pub mod engine;
pub mod gates;
pub mod handlers;
pub mod registry;

pub use engine::SanchoEngine;
pub use gates::{
    ActiveHoursGate, Gate, GateState, LockGate, SessionGate, TimeGate, WeekdayGate,
};
pub use handlers::{
    DailyBriefingHandler, DreamHandler, DynamicChecklistHandler, FakeLlmCall,
    IndexRebuildHandler, LlmCall, MemoryConsolidationHandler, MemoryPromotionHandler,
    SubprocessHandler, SuperbrainSyncEmbedHandler, WikiLintHandler,
};
pub use registry::{HandlerReport, SanchoContext, SanchoHandler, SanchoRegistry, TaskRegistration};

use std::sync::Arc;
use std::time::Duration;

/// Build the default production registry matching the Python SANCHO
/// defaults. Callers can append custom tasks to the returned registry
/// before handing it to [`SanchoEngine::new`].
pub fn default_registry(ctx: Arc<SanchoContext>) -> SanchoRegistry {
    let mut reg = SanchoRegistry::new();
    let llm_for_dream: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;
    let llm_for_brief: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;
    let llm_for_check: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;

    reg.register(
        Arc::new(DreamHandler::new(llm_for_dream)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(4 * 3600))),
            Arc::new(SessionGate),
            Arc::new(LockGate),
        ],
    );
    reg.register(
        Arc::new(WikiLintHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(6 * 3600)))],
    );
    reg.register(
        Arc::new(IndexRebuildHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(12 * 3600)))],
    );
    reg.register(
        Arc::new(DailyBriefingHandler::new(llm_for_brief)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(8 * 3600))),
            Arc::new(ActiveHoursGate::new(7, 22)),
        ],
    );
    reg.register(
        Arc::new(MemoryConsolidationHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(4 * 3600)))],
    );
    reg.register(
        Arc::new(MemoryPromotionHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(20 * 3600)))],
    );
    reg.register(
        Arc::new(SuperbrainSyncEmbedHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(12 * 60)))],
    );
    reg.register(
        Arc::new(DynamicChecklistHandler::new(llm_for_check)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(3600))),
            Arc::new(ActiveHoursGate::new(8, 22)),
        ],
    );

    // ─────────────────────────────────────────────────────────────
    // Legacy subprocess tasks (ported from crontab 2026-04-15)
    // These wrap existing Python scripts until they're rewritten in
    // native Rust. Each matches its original crontab cadence.
    // ─────────────────────────────────────────────────────────────
    let python = "/usr/local/opt/python@3.11/bin/python3.11".to_string();

    // switchAILocal watchdog — was every 5 min in crontab
    reg.register(
        Arc::new(SubprocessHandler::new(
            "switchailocal_watchdog",
            python.clone(),
            vec![
                "-u".to_string(),
                ctx.home
                    .join("harvey-os/core/watchdogs/switchailocal_watchdog.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )),
        vec![Arc::new(TimeGate::new(Duration::from_secs(5 * 60)))],
    );

    // Postgres cluster watchdog — was every 15 min
    reg.register(
        Arc::new(SubprocessHandler::new(
            "pg_watchdog",
            python.clone(),
            vec![
                "-u".to_string(),
                ctx.home
                    .join("agents/pg-watchdog/pg_watchdog.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )),
        vec![Arc::new(TimeGate::new(Duration::from_secs(15 * 60)))],
    );

    // HackerNews monitor — was every hour at :05
    reg.register(
        Arc::new(SubprocessHandler::new(
            "hackernews_monitor",
            python.clone(),
            vec![
                "-u".to_string(),
                ctx.home
                    .join("harvey-os/agents/hackernews/hn_monitor.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )),
        vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
    );

    // ─────────────────────────────────────────────────────────────
    // Harvey's Mascot GYM — 5 layers (2026-04-15 supercenter reopen)
    // All five layers route through the Python dispatch shim because
    // the GYM codebase is Python-first. Rust owns the schedule; Python
    // owns the work. Same SubprocessHandler pattern used above.
    // ─────────────────────────────────────────────────────────────
    let gym_shim = ctx
        .home
        .join("harvey-os/bin/run-sancho-task.py")
        .to_string_lossy()
        .into_owned();

    // gym_classify — hourly error cluster refresh (Layer 2)
    reg.register(
        Arc::new(SanchoSubprocess::gym("gym_classify", &python, &gym_shim)),
        vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
    );

    // gym_hypothesize — nightly (23.5h) hypothesis generation (Layer 3)
    // active 01:00-04:00 so it overlaps Sebastian's sleep window
    reg.register(
        Arc::new(SanchoSubprocess::gym("gym_hypothesize", &python, &gym_shim)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(84600))),
            Arc::new(ActiveHoursGate::new(1, 4)),
        ],
    );

    // gym_lope_gate — nightly lope validation (Layer 4)
    reg.register(
        Arc::new(SanchoSubprocess::gym("gym_lope_gate", &python, &gym_shim)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(84600))),
            Arc::new(ActiveHoursGate::new(3, 6)),
        ],
    );

    // gym_morning_report — daily Brain journal rollup (Layer 4b)
    reg.register(
        Arc::new(SanchoSubprocess::gym("gym_morning_report", &python, &gym_shim)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(84600))),
            Arc::new(ActiveHoursGate::new(6, 9)),
        ],
    );

    // gym_weekly_report — 7-day rollup + blocklist refresh (Level 2 supercenter)
    reg.register(
        Arc::new(SanchoSubprocess::gym("gym_weekly_report", &python, &gym_shim)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(7 * 86400))),
            Arc::new(ActiveHoursGate::new(8, 11)),
        ],
    );

    reg
}

/// Tiny ergonomic wrapper so the five gym tasks don't each repeat the
/// `python -u <shim> --task <name>` vector literal. Returns a
/// SubprocessHandler ready to register.
struct SanchoSubprocess;
impl SanchoSubprocess {
    fn gym(task: &str, python: &str, shim: &str) -> SubprocessHandler {
        SubprocessHandler::new(
            task,
            python.to_string(),
            vec![
                "-u".to_string(),
                shim.to_string(),
                "--task".to_string(),
                task.to_string(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::EmbeddingClient;
    use crate::event_bus::PersistentEventBus;
    use crate::llm::LlmClient;
    use crate::superbrain::store::SuperbrainStore;
    use tempfile::TempDir;

    #[test]
    fn default_registry_has_sixteen_tasks() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(SuperbrainStore::open(&dir.path().join("b.db")).unwrap());
        let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        let ctx = Arc::new(SanchoContext::new(
            store,
            bus,
            llm,
            emb,
            dir.path().to_path_buf(),
        ));
        let reg = default_registry(ctx);
        // 8 native Rust handlers + 3 legacy subprocess handlers + 5 gym tasks = 16
        assert_eq!(reg.len(), 16);
    }
}
