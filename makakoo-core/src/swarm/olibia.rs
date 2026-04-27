//! Olibia subagent — passive event listener that sprinkles personality
//! across the running system.
//!
//! Subscribes to `sancho.*`, `superbrain.*`, `swarm.*`, and `nursery.*`
//! on the persistent event bus (crossbeam-channel path) and reacts to
//! each event with a lightweight owl-voiced log line, plus occasional
//! gimmick frames via `gimmicks::render_gimmick`.
//!
//! Ported from `core/subagents/olibia_agent.py`. Reactions stay *light*:
//! no LLM calls inside the event loop, just shape a one-liner and log
//! it at `INFO`. The LLM handle is kept around so future mode switches
//! (e.g. "summarise the last minute of activity every tick") can reach
//! for it without re-plumbing.
//!
//! Run via [`OlibiaSubagent::run_forever`] from a dedicated tokio task.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::event_bus::{Event, PersistentEventBus};
use crate::gimmicks;
use crate::llm::LlmClient;

/// Default channel buffer size for the Olibia subscriber.
const OLIBIA_CHANNEL_CAP: usize = 512;

/// Typed view over the events Olibia cares about. Used by the MCP-side
/// handlers / tests that want to tag events without pattern-matching
/// strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OlibiaEvent {
    SanchoTick(Value),
    SuperbrainWrite(Value),
    SwarmDispatch(Value),
    NurseryHatch(Value),
    Other(String, Value),
}

impl OlibiaEvent {
    pub fn from_bus_event(ev: &Event) -> Self {
        let data = ev.data.clone();
        if ev.topic.starts_with("sancho.") {
            OlibiaEvent::SanchoTick(data)
        } else if ev.topic.starts_with("superbrain.") {
            OlibiaEvent::SuperbrainWrite(data)
        } else if ev.topic.starts_with("swarm.") {
            OlibiaEvent::SwarmDispatch(data)
        } else if ev.topic.starts_with("nursery.") {
            OlibiaEvent::NurseryHatch(data)
        } else {
            OlibiaEvent::Other(ev.topic.clone(), data)
        }
    }

    /// Build an owl-voiced one-liner. Deliberately tiny — the goal is
    /// a log trace, not an LLM conversation.
    pub fn owl_voice(&self) -> String {
        match self {
            OlibiaEvent::SanchoTick(_) => "hoot — sancho did a tick".to_string(),
            OlibiaEvent::SuperbrainWrite(_) => {
                "hoot — brain grew a new leaf".to_string()
            }
            OlibiaEvent::SwarmDispatch(_) => "hoot — swarm swung into motion".to_string(),
            OlibiaEvent::NurseryHatch(_) => "hoot — a new chick hatched".to_string(),
            OlibiaEvent::Other(topic, _) => format!("hoot — heard {topic}"),
        }
    }
}

/// The Olibia subscriber. Constructed by [`Self::new`]; drive to
/// completion via [`Self::run_forever`] on a dedicated tokio task.
pub struct OlibiaSubagent {
    #[allow(dead_code)]
    bus: Arc<PersistentEventBus>,
    #[allow(dead_code)]
    llm: Arc<LlmClient>,
    rx: Receiver<Event>,
    reaction_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl OlibiaSubagent {
    /// Register the bus subscriber and return a driver.
    ///
    /// Uses one channel with a combined `*` subscription set up via
    /// multiple calls to `subscribe_channel`. Because each call produces
    /// its own receiver, we instead use `subscribe` with a relay closure
    /// that forwards into a single sender. This keeps the reaction loop
    /// cleanly `select!`-able.
    pub fn new(bus: Arc<PersistentEventBus>, llm: Arc<LlmClient>) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<Event>(OLIBIA_CHANNEL_CAP);
        for pattern in ["sancho.*", "superbrain.*", "swarm.*", "nursery.*"] {
            let tx = tx.clone();
            bus.subscribe(pattern, move |ev| {
                // Drop-on-full so we never stall the publisher.
                let _ = tx.try_send(ev.clone());
            });
        }
        Self {
            bus,
            llm,
            rx,
            reaction_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// React to one pending event without blocking. Returns the reaction
    /// string if an event was available. Exposed for tests and for
    /// tight-loop integration with a shared tokio runtime.
    pub fn react_once(&self) -> Option<String> {
        let ev = self.rx.try_recv().ok()?;
        self.react_to(&ev)
    }

    /// React to a specific event. Split out so tests can inject events
    /// directly instead of going through the bus.
    pub fn react_to(&self, ev: &Event) -> Option<String> {
        let olibia_ev = OlibiaEvent::from_bus_event(ev);
        let line = olibia_ev.owl_voice();
        let n = self
            .reaction_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        tracing::info!(target: "makakoo.olibia", topic = %ev.topic, "{line}");
        // Every 7th reaction, attempt a gimmick frame — cooldown-gated
        // inside render_gimmick so this is safe to call on every call.
        if n % 7 == 0 {
            match gimmicks::render_gimmick(&ev.topic, false) {
                Ok(Some(frame)) => {
                    tracing::debug!(
                        target: "makakoo.olibia",
                        "gimmick frame rendered ({} chars)",
                        frame.len()
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(
                        target: "makakoo.olibia",
                        "gimmick render failed: {e}"
                    );
                }
            }
        }
        Some(line)
    }

    /// Drive Olibia forever. Each iteration blocks on the crossbeam
    /// receiver via a `tokio::task::spawn_blocking` shim so we don't
    /// starve the async runtime. Exits cleanly when every sender has
    /// dropped (i.e. the bus was torn down).
    pub async fn run_forever(self) -> Result<()> {
        loop {
            let rx = self.rx.clone();
            let ev = tokio::task::spawn_blocking(move || rx.recv()).await;
            match ev {
                Ok(Ok(ev)) => {
                    self.react_to(&ev);
                }
                Ok(Err(_)) => {
                    // All senders dropped — shut down cleanly.
                    tracing::info!(
                        target: "makakoo.olibia",
                        "event bus closed; olibia exiting"
                    );
                    return Ok(());
                }
                Err(join_err) => {
                    tracing::warn!(
                        target: "makakoo.olibia",
                        "receiver task failed: {join_err}"
                    );
                    return Ok(());
                }
            }
        }
    }

    /// Total reactions processed so far.
    pub fn reaction_count(&self) -> usize {
        self.reaction_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build_subagent() -> (tempfile::TempDir, Arc<PersistentEventBus>, OlibiaSubagent) {
        let dir = tempfile::tempdir().unwrap();
        let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
        let llm = Arc::new(LlmClient::new());
        let olibia = OlibiaSubagent::new(Arc::clone(&bus), llm);
        (dir, bus, olibia)
    }

    #[test]
    fn reacts_to_sancho_tick() {
        let (_d, bus, olibia) = build_subagent();
        bus.publish("sancho.tick", "test", json!({"id": 1})).unwrap();
        let line = olibia.react_once().unwrap();
        assert!(line.contains("sancho"));
        assert_eq!(olibia.reaction_count(), 1);
    }

    #[test]
    fn reacts_to_swarm_dispatch() {
        let (_d, bus, olibia) = build_subagent();
        bus.publish(
            "swarm.dispatch.complete",
            "gw",
            json!({"run_id": "r1"}),
        )
        .unwrap();
        let line = olibia.react_once().unwrap();
        assert!(line.contains("swarm"));
    }

    #[test]
    fn ignores_unrelated_topics() {
        let (_d, bus, olibia) = build_subagent();
        bus.publish("unrelated.topic", "x", json!({})).unwrap();
        // Not subscribed — nothing to pull.
        assert!(olibia.react_once().is_none());
    }

    #[test]
    fn owl_voice_covers_all_variants() {
        let ev_fn = |topic: &str| Event {
            seq: 1,
            topic: topic.to_string(),
            source: "t".to_string(),
            data: Value::Null,
            timestamp: 0.0,
        };
        assert!(OlibiaEvent::from_bus_event(&ev_fn("sancho.a"))
            .owl_voice()
            .contains("sancho"));
        assert!(OlibiaEvent::from_bus_event(&ev_fn("superbrain.write"))
            .owl_voice()
            .contains("brain"));
        assert!(OlibiaEvent::from_bus_event(&ev_fn("swarm.dispatch.start"))
            .owl_voice()
            .contains("swarm"));
        assert!(OlibiaEvent::from_bus_event(&ev_fn("nursery.hatch"))
            .owl_voice()
            .contains("chick"));
        assert!(OlibiaEvent::from_bus_event(&ev_fn("other.thing"))
            .owl_voice()
            .contains("other.thing"));
    }
}
