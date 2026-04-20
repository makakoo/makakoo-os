//! SANCHO engine — tick loop that runs eligible tasks and publishes
//! reports to the event bus.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tokio::time::interval;
use tracing::{error, info};

use crate::error::Result;
use crate::sancho::gates::GateState;
use crate::sancho::registry::{HandlerReport, SanchoContext, SanchoRegistry};

/// Default cadence for the `sancho tick: N/M tasks ok` heartbeat when the
/// daemon would otherwise go silent on an all-gated tick. Chosen to match
/// the sancho-watchdog escalation window so a missed heartbeat means
/// something went wrong, not that the loop is just idle.
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Running counters the `run_forever` loop flushes on each heartbeat.
#[derive(Debug)]
struct HeartbeatState {
    last_emit: Instant,
    ok_count: u32,
    total_count: u32,
    ticks: u32,
}

impl HeartbeatState {
    fn new() -> Self {
        Self {
            last_emit: Instant::now(),
            ok_count: 0,
            total_count: 0,
            ticks: 0,
        }
    }
}

/// Proactive task engine.
pub struct SanchoEngine {
    registry: SanchoRegistry,
    state: Arc<Mutex<GateState>>,
    ctx: Arc<SanchoContext>,
    tick_interval: Duration,
    heartbeat_interval: Duration,
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
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Override the heartbeat emit cadence. Primarily exists so tests can
    /// assert heartbeat behaviour in sub-second windows without waiting 5
    /// minutes. Production callers should stick with the default.
    #[must_use]
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = interval;
        self
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
    ///
    /// Emits a `sancho tick: <ok>/<total> tasks ok` INFO line every
    /// `heartbeat_interval` so `tail -f makakoo.err.log` shows the daemon
    /// is alive even when no tasks fire. Closes the "alive but quiet"
    /// blind spot that hid a 30-hour 401 storm 2026-04-18.
    pub async fn run_forever(self) -> Result<()> {
        let mut ticker = interval(self.tick_interval);
        ticker.tick().await; // Drop the immediate first tick.
        info!(
            tick_interval_sec = self.tick_interval.as_secs_f64(),
            heartbeat_interval_sec = self.heartbeat_interval.as_secs_f64(),
            task_count = self.task_count(),
            "sancho: engine started"
        );
        let mut hb = HeartbeatState::new();
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    hb.ticks += 1;
                    match self.tick_once().await {
                        Ok(reports) => {
                            hb.total_count += reports.len() as u32;
                            hb.ok_count += reports.iter().filter(|r| r.ok).count() as u32;
                        }
                        Err(e) => {
                            error!(error = %e, "sancho: tick failed");
                        }
                    }
                    if hb.last_emit.elapsed() >= self.heartbeat_interval {
                        info!(
                            ok = hb.ok_count,
                            total = hb.total_count,
                            ticks = hb.ticks,
                            "sancho tick: {}/{} tasks ok",
                            hb.ok_count,
                            hb.total_count,
                        );
                        hb = HeartbeatState::new();
                    }
                }
                _ = self.shutdown.notified() => {
                    info!(
                        final_ok = hb.ok_count,
                        final_total = hb.total_count,
                        final_ticks = hb.ticks,
                        "sancho: shutdown requested"
                    );
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

    /// Sync buffer the tracing subscriber writes into. Using
    /// `std::sync::Mutex` (not tokio's) keeps the write path fully
    /// blocking, as tracing expects, without pulling in `futures`.
    #[derive(Clone)]
    struct CapturingWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for CapturingWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let mut guard = self
                .0
                .lock()
                .expect("capture buffer mutex poisoned");
            guard.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_forever_emits_heartbeat_on_elapsed_interval() {
        let dir = TempDir::new().unwrap();
        let ctx = test_ctx(&dir);
        let count = Arc::new(AtomicUsize::new(0));
        let mut registry = SanchoRegistry::new();
        registry.register(
            Arc::new(CountingHandler {
                name_val: "hb_tick".into(),
                count: Arc::clone(&count),
            }),
            Vec::new(),
        );

        // Capture INFO-level events so we can assert the heartbeat fires.
        let buf: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = CapturingWriter(Arc::clone(&buf));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish();
        let _dispatch = tracing::subscriber::set_default(subscriber);

        let engine = SanchoEngine::new(registry, ctx, Duration::from_millis(20))
            .with_heartbeat_interval(Duration::from_millis(100));
        let notify = engine.shutdown_handle();
        let jh = tokio::spawn(async move {
            engine.run_forever().await.unwrap();
        });

        // Give the loop enough wall-clock to tick multiple times + cross
        // at least one heartbeat threshold.
        tokio::time::sleep(Duration::from_millis(300)).await;
        notify.notify_one();
        tokio::time::timeout(Duration::from_secs(2), jh)
            .await
            .expect("run_forever did not exit after shutdown")
            .unwrap();

        let captured = {
            let guard = buf.lock().unwrap();
            String::from_utf8_lossy(&guard).to_string()
        };
        // At least one heartbeat should have fired in 300 ms with a
        // 100 ms interval. The startup banner is emitted inside the
        // spawned task; tokio's test runtime swaps thread-local tracing
        // dispatchers on the very first poll of that task, so we don't
        // assert on it here (covered by the live daemon). Heartbeats
        // emit later, after the dispatcher re-settles on the test
        // thread, which is what we actually want to lock down.
        assert!(
            captured.contains("sancho tick:") && captured.contains("tasks ok"),
            "expected heartbeat line in captured logs, got: {captured}"
        );
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
