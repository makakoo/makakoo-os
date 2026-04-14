//! SANCHO engine — tick loop that runs eligible tasks and publishes
//! reports to the event bus.

use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tokio::time::interval;
use tracing::{error, info};

use crate::error::Result;
use crate::sancho::gates::GateState;
use crate::sancho::registry::{HandlerReport, SanchoContext, SanchoRegistry};

/// Proactive task engine.
pub struct SanchoEngine {
    registry: SanchoRegistry,
    state: Arc<Mutex<GateState>>,
    ctx: Arc<SanchoContext>,
    tick_interval: Duration,
    shutdown: Arc<Notify>,
}

impl SanchoEngine {
    /// Build a new engine. The engine takes ownership of the registry.
    pub fn new(
        registry: SanchoRegistry,
        ctx: Arc<SanchoContext>,
        tick_interval: Duration,
    ) -> Self {
        Self {
            registry,
            state: Arc::new(Mutex::new(GateState::new())),
            ctx,
            tick_interval,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Shared handle to gate state.
    pub fn state(&self) -> Arc<Mutex<GateState>> {
        Arc::clone(&self.state)
    }

    /// Number of registered tasks.
    pub fn task_count(&self) -> usize {
        self.registry.len()
    }

    /// Run every eligible task exactly once.
    pub async fn tick_once(&self) -> Result<Vec<HandlerReport>> {
        let now = Local::now();
        let mut reports = Vec::new();

        // Snapshot eligible tasks + acquire locks under one critical
        // section so concurrent ticks can't double-run a task.
        let eligible: Vec<usize> = {
            let mut state = self.state.lock().await;
            let mut picks = Vec::new();
            for (idx, task) in self.registry.tasks().iter().enumerate() {
                let name = task.handler.name();
                let allowed = task.gates.iter().all(|g| g.allows(name, now, &state));
                if !allowed {
                    continue;
                }
                if !state.try_acquire(name) {
                    continue;
                }
                picks.push(idx);
            }
            picks
        };

        for idx in eligible {
            let task = &self.registry.tasks()[idx];
            let name = task.handler.name().to_string();
            info!(task = %name, "sancho: running handler");
            let report = match task.handler.run(&self.ctx).await {
                Ok(r) => r,
                Err(e) => {
                    error!(task = %name, error = %e, "sancho: handler failed");
                    HandlerReport::failed(&name, format!("{e}"), Duration::ZERO)
                }
            };
            {
                let mut state = self.state.lock().await;
                state.release(&name);
                if report.ok {
                    state.record_run(&name, Local::now());
                }
            }
            let _ = self.ctx.bus.publish(
                "sancho.handler.tick",
                "sancho",
                json!({
                    "handler": report.handler,
                    "ok": report.ok,
                    "message": report.message,
                    "duration_sec": report.duration.as_secs_f64(),
                }),
            );
            reports.push(report);
        }

        let _ = self.ctx.bus.publish(
            "sancho.tick",
            "sancho",
            json!({
                "count": reports.len(),
                "total_duration_sec": reports
                    .iter()
                    .map(|r| r.duration.as_secs_f64())
                    .sum::<f64>(),
            }),
        );

        Ok(reports)
    }

    /// Run forever, ticking on `tick_interval`. Stops on shutdown signal.
    pub async fn run_forever(self) -> Result<()> {
        let mut ticker = interval(self.tick_interval);
        ticker.tick().await; // Drop the immediate first tick.
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = self.tick_once().await {
                        error!(error = %e, "sancho: tick failed");
                    }
                }
                _ = self.shutdown.notified() => {
                    info!("sancho: shutdown requested");
                    return Ok(());
                }
            }
        }
    }

    /// Signal a running `run_forever` loop to exit.
    pub async fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Shared shutdown notifier (tests).
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown)
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
    use crate::sancho::gates::TimeGate;
    use crate::sancho::registry::{HandlerReport, SanchoHandler};
    use crate::superbrain::store::SuperbrainStore;
    use async_trait::async_trait;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct CountingHandler {
        name_val: String,
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SanchoHandler for CountingHandler {
        fn name(&self) -> &str {
            &self.name_val
        }
        async fn run(&self, _ctx: &SanchoContext) -> Result<HandlerReport> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(HandlerReport::ok(
                &self.name_val,
                "counted",
                Duration::from_millis(1),
            ))
        }
    }

    struct FailingHandler;

    #[async_trait]
    impl SanchoHandler for FailingHandler {
        fn name(&self) -> &str {
            "boom"
        }
        async fn run(&self, _ctx: &SanchoContext) -> Result<HandlerReport> {
            Err(crate::error::MakakooError::internal("synthetic boom"))
        }
    }

    fn test_ctx(dir: &TempDir) -> Arc<SanchoContext> {
        let store = Arc::new(SuperbrainStore::open(&dir.path().join("b.db")).unwrap());
        let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        let home: PathBuf = dir.path().join("home");
        fs::create_dir_all(home.join("data").join("Brain").join("journals")).unwrap();
        Arc::new(SanchoContext::new(store, bus, llm, emb, home))
    }

    #[tokio::test]
    async fn tick_runs_eligible_tasks_once() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);

        let count = Arc::new(AtomicUsize::new(0));
        let mut registry = SanchoRegistry::new();
        registry.register(
            Arc::new(CountingHandler {
                name_val: "tick_test".into(),
                count: Arc::clone(&count),
            }),
            vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
        );

        let engine = SanchoEngine::new(registry, ctx, Duration::from_millis(50));
        let reports = engine.tick_once().await.unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].ok);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_tick_gates_out_if_interval_unmet() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);

        let count = Arc::new(AtomicUsize::new(0));
        let mut registry = SanchoRegistry::new();
        registry.register(
            Arc::new(CountingHandler {
                name_val: "gated".into(),
                count: Arc::clone(&count),
            }),
            vec![Arc::new(TimeGate::new(Duration::from_secs(3600)))],
        );

        let engine = SanchoEngine::new(registry, ctx, Duration::from_millis(50));
        let _ = engine.tick_once().await.unwrap();
        let second = engine.tick_once().await.unwrap();
        assert!(
            second.is_empty(),
            "second tick should be gated out by TimeGate"
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failing_handler_produces_failed_report_and_releases_lock() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);

        let mut registry = SanchoRegistry::new();
        registry.register(Arc::new(FailingHandler), Vec::new());

        let engine = SanchoEngine::new(registry, ctx, Duration::from_millis(50));
        let reports = engine.tick_once().await.unwrap();
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].ok);

        let state = engine.state();
        let guard = state.lock().await;
        assert_eq!(guard.locks.get("boom").copied(), Some(false));
    }

    #[tokio::test]
    async fn run_forever_exits_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);
        let mut registry = SanchoRegistry::new();
        let count = Arc::new(AtomicUsize::new(0));
        registry.register(
            Arc::new(CountingHandler {
                name_val: "loop_test".into(),
                count: Arc::clone(&count),
            }),
            Vec::new(),
        );
        let engine = SanchoEngine::new(registry, ctx, Duration::from_millis(20));
        let notify = engine.shutdown_handle();
        let jh = tokio::spawn(async move {
            engine.run_forever().await.unwrap();
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        // `notify_one` stores a permit so the signal isn't lost if the
        // loop hasn't yet hit its `notified().await` call.
        notify.notify_one();
        tokio::time::timeout(Duration::from_secs(2), jh)
            .await
            .expect("run_forever did not exit after shutdown")
            .unwrap();
        assert!(count.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn tick_publishes_sancho_tick_event() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);
        let mut registry = SanchoRegistry::new();
        registry.register(
            Arc::new(CountingHandler {
                name_val: "evt".into(),
                count: Arc::new(AtomicUsize::new(0)),
            }),
            Vec::new(),
        );
        let engine = SanchoEngine::new(registry, Arc::clone(&ctx), Duration::from_millis(50));
        let _ = engine.tick_once().await.unwrap();
        let events = ctx.bus.recent(10, "sancho.*").unwrap();
        assert!(events.iter().any(|e| e.topic == "sancho.tick"));
    }
}
