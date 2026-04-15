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
    // Optional subprocess tasks (Python workshop on top of Rust core)
    //
    // These wrap Python scripts that live in the sibling harvey-os
    // tree and under agents/. None of them are required for the Rust
    // daemon to boot — each registration checks `script.exists()`
    // first and is silently skipped if missing. That way a fresh
    // public install of makakoo-os runs cleanly with just the 8
    // native Rust tasks, and upgrading to a full-fat install (cloning
    // harvey-os alongside) lights up the extra tasks automatically on
    // next daemon restart. Ported from crontab 2026-04-15; graceful
    // degrade added 2026-04-15 as part of the GYM supercenter work.
    // ─────────────────────────────────────────────────────────────
    let python = "/usr/local/opt/python@3.11/bin/python3.11".to_string();

    let switchailocal_script = ctx.home.join("harvey-os/core/watchdogs/switchailocal_watchdog.py");
    if switchailocal_script.exists() {
        reg.register(
            Arc::new(SubprocessHandler::new(
                "switchailocal_watchdog",
                python.clone(),
                vec![
                    "-u".to_string(),
                    switchailocal_script.to_string_lossy().into_owned(),
                ],
            )),
            vec![Arc::new(TimeGate::new(Duration::from_secs(5 * 60)))],
        );
    }

    let pg_script = ctx.home.join("agents/pg-watchdog/pg_watchdog.py");
    if pg_script.exists() {
        reg.register(
            Arc::new(SubprocessHandler::new(
                "pg_watchdog",
                python.clone(),
                vec!["-u".to_string(), pg_script.to_string_lossy().into_owned()],
            )),
            vec![Arc::new(TimeGate::new(Duration::from_secs(15 * 60)))],
        );
    }

    let hn_script = ctx.home.join("harvey-os/agents/hackernews/hn_monitor.py");
    if hn_script.exists() {
        reg.register(
            Arc::new(SubprocessHandler::new(
                "hackernews_monitor",
                python.clone(),
                vec!["-u".to_string(), hn_script.to_string_lossy().into_owned()],
            )),
            vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
        );
    }

    // Harvey's Mascot GYM — 5 layers, all through one dispatch shim.
    // Register all five iff the shim exists; otherwise skip the whole
    // GYM. A partial GYM is worse than no GYM.
    let gym_shim_path = ctx.home.join("harvey-os/bin/run-sancho-task.py");
    if gym_shim_path.exists() {
        let gym_shim = gym_shim_path.to_string_lossy().into_owned();

        reg.register(
            Arc::new(SanchoSubprocess::gym("gym_classify", &python, &gym_shim)),
            vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
        );

        reg.register(
            Arc::new(SanchoSubprocess::gym("gym_hypothesize", &python, &gym_shim)),
            vec![
                Arc::new(TimeGate::new(Duration::from_secs(84600))),
                Arc::new(ActiveHoursGate::new(1, 4)),
            ],
        );

        reg.register(
            Arc::new(SanchoSubprocess::gym("gym_lope_gate", &python, &gym_shim)),
            vec![
                Arc::new(TimeGate::new(Duration::from_secs(84600))),
                Arc::new(ActiveHoursGate::new(3, 6)),
            ],
        );

        reg.register(
            Arc::new(SanchoSubprocess::gym("gym_morning_report", &python, &gym_shim)),
            vec![
                Arc::new(TimeGate::new(Duration::from_secs(84600))),
                Arc::new(ActiveHoursGate::new(6, 9)),
            ],
        );

        reg.register(
            Arc::new(SanchoSubprocess::gym("gym_weekly_report", &python, &gym_shim)),
            vec![
                Arc::new(TimeGate::new(Duration::from_secs(7 * 86400))),
                Arc::new(ActiveHoursGate::new(8, 11)),
            ],
        );
    }

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

    fn make_ctx(home: &std::path::Path) -> Arc<SanchoContext> {
        let store = Arc::new(SuperbrainStore::open(&home.join("b.db")).unwrap());
        let bus = PersistentEventBus::open(&home.join("bus.db")).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        Arc::new(SanchoContext::new(store, bus, llm, emb, home.to_path_buf()))
    }

    #[test]
    fn fresh_install_registers_only_native_tasks() {
        // No harvey-os/, no agents/. Graceful degrade: only the 8 native
        // Rust handlers should register. This is what a fresh public
        // install of makakoo-os looks like.
        let dir = TempDir::new().unwrap();
        let reg = default_registry(make_ctx(dir.path()));
        assert_eq!(
            reg.len(),
            8,
            "fresh install with no Python scripts should yield exactly 8 native tasks"
        );
    }

    #[test]
    fn full_install_registers_sixteen_tasks() {
        // Seed the expected script paths as empty stub files. With all
        // 4 optional scripts present, we get the full 16-task surface:
        // 8 native + 3 watchdogs + 5 gym.
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let touch = |rel: &str| {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"#!/usr/bin/env python3\n").unwrap();
        };
        touch("harvey-os/core/watchdogs/switchailocal_watchdog.py");
        touch("agents/pg-watchdog/pg_watchdog.py");
        touch("harvey-os/agents/hackernews/hn_monitor.py");
        touch("harvey-os/bin/run-sancho-task.py");

        let reg = default_registry(make_ctx(home));
        assert_eq!(
            reg.len(),
            16,
            "full install with all Python scripts present should yield 16 tasks"
        );
    }

    #[test]
    fn partial_install_gym_skipped_when_shim_missing() {
        // Watchdogs present but the GYM dispatch shim is not.
        // Expect 8 native + 3 watchdogs = 11, with no gym_* at all.
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let touch = |rel: &str| {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"#!/usr/bin/env python3\n").unwrap();
        };
        touch("harvey-os/core/watchdogs/switchailocal_watchdog.py");
        touch("agents/pg-watchdog/pg_watchdog.py");
        touch("harvey-os/agents/hackernews/hn_monitor.py");

        let reg = default_registry(make_ctx(home));
        assert_eq!(reg.len(), 11);
    }
}
