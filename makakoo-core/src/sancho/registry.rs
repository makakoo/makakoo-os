//! SANCHO task registry + handler trait.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingClient;
use crate::error::Result;
use crate::event_bus::PersistentEventBus;
use crate::llm::LlmClient;
use crate::sancho::gates::Gate;
use crate::superbrain::store::SuperbrainStore;

/// Proactive task trait. Handlers are short, async, side-effecting.
#[async_trait]
pub trait SanchoHandler: Send + Sync {
    fn name(&self) -> &str;
    async fn run(&self, ctx: &SanchoContext) -> Result<HandlerReport>;
}

/// Dependency bundle passed to every handler on each tick.
pub struct SanchoContext {
    pub store: Arc<SuperbrainStore>,
    pub bus: Arc<PersistentEventBus>,
    pub llm: Arc<LlmClient>,
    pub emb: Arc<EmbeddingClient>,
    pub home: PathBuf,
}

impl SanchoContext {
    pub fn new(
        store: Arc<SuperbrainStore>,
        bus: Arc<PersistentEventBus>,
        llm: Arc<LlmClient>,
        emb: Arc<EmbeddingClient>,
        home: PathBuf,
    ) -> Self {
        Self {
            store,
            bus,
            llm,
            emb,
            home,
        }
    }
}

/// Result emitted by a single handler invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerReport {
    pub handler: String,
    pub ok: bool,
    pub message: String,
    #[serde(with = "duration_seconds")]
    pub duration: Duration,
}

impl HandlerReport {
    pub fn ok(handler: impl Into<String>, message: impl Into<String>, duration: Duration) -> Self {
        Self {
            handler: handler.into(),
            ok: true,
            message: message.into(),
            duration,
        }
    }

    pub fn failed(
        handler: impl Into<String>,
        message: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            handler: handler.into(),
            ok: false,
            message: message.into(),
            duration,
        }
    }
}

mod duration_seconds {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs_f64().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(d)?;
        Ok(Duration::from_secs_f64(secs.max(0.0)))
    }
}

/// One handler + its guard gates.
#[derive(Clone)]
pub struct TaskRegistration {
    pub handler: Arc<dyn SanchoHandler>,
    pub gates: Vec<Arc<dyn Gate>>,
}

impl TaskRegistration {
    pub fn new(handler: Arc<dyn SanchoHandler>, gates: Vec<Arc<dyn Gate>>) -> Self {
        Self { handler, gates }
    }
}

/// Collection of registered tasks (insertion-ordered).
#[derive(Default, Clone)]
pub struct SanchoRegistry {
    tasks: Vec<TaskRegistration>,
}

impl SanchoRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn SanchoHandler>, gates: Vec<Arc<dyn Gate>>) {
        self.tasks.push(TaskRegistration { handler, gates });
    }

    pub fn tasks(&self) -> &[TaskRegistration] {
        &self.tasks
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct NopHandler;

    #[async_trait]
    impl SanchoHandler for NopHandler {
        fn name(&self) -> &str {
            "nop"
        }
        async fn run(&self, _ctx: &SanchoContext) -> Result<HandlerReport> {
            Ok(HandlerReport::ok("nop", "ok", Duration::from_millis(1)))
        }
    }

    #[test]
    fn registry_stores_tasks_in_order() {
        let mut reg = SanchoRegistry::new();
        reg.register(Arc::new(NopHandler), Vec::new());
        reg.register(Arc::new(NopHandler), Vec::new());
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.tasks()[0].handler.name(), "nop");
        assert!(!reg.is_empty());
    }

    #[test]
    fn handler_report_roundtrip_json() {
        let r = HandlerReport::ok("dream", "3 pages", Duration::from_millis(1500));
        let s = serde_json::to_string(&r).unwrap();
        let back: HandlerReport = serde_json::from_str(&s).unwrap();
        assert_eq!(back.handler, "dream");
        assert!(back.ok);
        assert!((back.duration.as_secs_f64() - 1.5).abs() < 1e-6);
    }
}
