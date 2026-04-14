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
    SuperbrainSyncEmbedHandler, WikiLintHandler,
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
    reg
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
    fn default_registry_has_eight_tasks() {
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
        assert_eq!(reg.len(), 8);
    }
}
