//! Lazy-initialized subsystem container for CLI commands.
//!
//! Opening the Brain DB, chat DB, and event bus on every invocation
//! would be wasteful — `makakoo version` has no reason to touch sqlite.
//! [`CliContext`] caches each subsystem in a [`OnceCell`] so the first
//! getter pays the open cost and subsequent calls are essentially free.
//!
//! A few getters aren't yet consumed by wave 5 subcommands (graph,
//! memory stack, chat store) — they're deliberately present so future
//! waves can wire tools like `chat send` or `graph query` without
//! reshaping the struct. The crate-level `#[allow(dead_code)]` in
//! main.rs covers that intentional API surface.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use once_cell::sync::OnceCell;

use makakoo_core::chat::ChatStore;
use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::nursery::{BuddyTracker, MascotRegistry};
use makakoo_core::platform::makakoo_home;
use makakoo_core::superbrain::graph::GraphStore;
use makakoo_core::superbrain::memory_stack::MemoryStack;
use makakoo_core::superbrain::promoter::MemoryPromoter;
use makakoo_core::superbrain::store::SuperbrainStore;

/// Bundle of lazily-opened subsystems the CLI commands share.
pub struct CliContext {
    home: PathBuf,
    data_dir: PathBuf,
    db_path: PathBuf,
    bus_path: PathBuf,
    chat_path: PathBuf,
    nursery_path: PathBuf,
    buddy_path: PathBuf,

    store: OnceCell<Arc<SuperbrainStore>>,
    graph: OnceCell<Arc<GraphStore>>,
    memory: OnceCell<Arc<MemoryStack>>,
    promoter: OnceCell<Arc<MemoryPromoter>>,
    bus: OnceCell<Arc<PersistentEventBus>>,
    chat: OnceCell<Arc<ChatStore>>,
    nursery: OnceCell<Arc<MascotRegistry>>,
    buddy: OnceCell<Arc<BuddyTracker>>,
    llm: OnceCell<Arc<LlmClient>>,
    emb: OnceCell<Arc<EmbeddingClient>>,
}

impl CliContext {
    /// Resolve $MAKAKOO_HOME and snapshot the canonical paths. No I/O
    /// happens here — every subsystem opens on first getter call.
    pub fn new() -> anyhow::Result<Self> {
        let home = makakoo_home();
        let data_dir = home.join("data");
        Ok(Self {
            home: home.clone(),
            db_path: data_dir.join("superbrain.db"),
            bus_path: data_dir.join("events.db"),
            chat_path: data_dir.join("chat.db"),
            nursery_path: data_dir.join("nursery.json"),
            buddy_path: data_dir.join("buddy.json"),
            data_dir,
            store: OnceCell::new(),
            graph: OnceCell::new(),
            memory: OnceCell::new(),
            promoter: OnceCell::new(),
            bus: OnceCell::new(),
            chat: OnceCell::new(),
            nursery: OnceCell::new(),
            buddy: OnceCell::new(),
            llm: OnceCell::new(),
            emb: OnceCell::new(),
        })
    }

    /// Canonical Makakoo home.
    pub fn home(&self) -> &PathBuf {
        &self.home
    }

    /// Canonical data dir (`{home}/data`).
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    /// Superbrain store — opens on first call.
    pub fn store(&self) -> anyhow::Result<Arc<SuperbrainStore>> {
        if let Some(s) = self.store.get() {
            return Ok(Arc::clone(s));
        }
        let store = SuperbrainStore::open(&self.db_path).with_context(|| {
            format!("failed to open superbrain store at {}", self.db_path.display())
        })?;
        let arc = Arc::new(store);
        let _ = self.store.set(Arc::clone(&arc));
        Ok(arc)
    }

    /// Graph store over the same connection as the superbrain store.
    pub fn graph(&self) -> anyhow::Result<Arc<GraphStore>> {
        if let Some(g) = self.graph.get() {
            return Ok(Arc::clone(g));
        }
        let store = self.store()?;
        let graph = Arc::new(GraphStore::new(store.conn_arc()));
        let _ = self.graph.set(Arc::clone(&graph));
        Ok(graph)
    }

    /// Memory stack — needs graph + store.
    pub fn memory(&self) -> anyhow::Result<Arc<MemoryStack>> {
        if let Some(m) = self.memory.get() {
            return Ok(Arc::clone(m));
        }
        let store = self.store()?;
        let graph = self.graph()?;
        let mem = Arc::new(MemoryStack::new(store.conn_arc(), graph));
        let _ = self.memory.set(Arc::clone(&mem));
        Ok(mem)
    }

    /// Memory promoter over the superbrain connection.
    pub fn promoter(&self) -> anyhow::Result<Arc<MemoryPromoter>> {
        if let Some(p) = self.promoter.get() {
            return Ok(Arc::clone(p));
        }
        let store = self.store()?;
        let p = Arc::new(MemoryPromoter::new(store.conn_arc()));
        let _ = self.promoter.set(Arc::clone(&p));
        Ok(p)
    }

    /// Persistent event bus. Opens `{data}/events.db`.
    pub fn event_bus(&self) -> anyhow::Result<Arc<PersistentEventBus>> {
        if let Some(b) = self.bus.get() {
            return Ok(Arc::clone(b));
        }
        let bus = PersistentEventBus::open(&self.bus_path)
            .with_context(|| format!("failed to open event bus at {}", self.bus_path.display()))?;
        let _ = self.bus.set(Arc::clone(&bus));
        Ok(bus)
    }

    /// Chat store. Opens `{data}/chat.db`.
    pub fn chat(&self) -> anyhow::Result<Arc<ChatStore>> {
        if let Some(c) = self.chat.get() {
            return Ok(Arc::clone(c));
        }
        let chat = ChatStore::open(&self.chat_path)
            .with_context(|| format!("failed to open chat store at {}", self.chat_path.display()))?;
        let arc = Arc::new(chat);
        let _ = self.chat.set(Arc::clone(&arc));
        Ok(arc)
    }

    /// Mascot registry — seeds canonical mascots on first load.
    pub fn nursery(&self) -> anyhow::Result<Arc<MascotRegistry>> {
        if let Some(n) = self.nursery.get() {
            return Ok(Arc::clone(n));
        }
        let reg = MascotRegistry::load(&self.nursery_path).with_context(|| {
            format!("failed to load nursery at {}", self.nursery_path.display())
        })?;
        let arc = Arc::new(reg);
        let _ = self.nursery.set(Arc::clone(&arc));
        Ok(arc)
    }

    /// Buddy tracker — depends on the mascot registry.
    pub fn buddy(&self) -> anyhow::Result<Arc<BuddyTracker>> {
        if let Some(b) = self.buddy.get() {
            return Ok(Arc::clone(b));
        }
        let registry = self.nursery()?;
        let tracker = BuddyTracker::load(registry, &self.buddy_path).with_context(|| {
            format!("failed to load buddy tracker at {}", self.buddy_path.display())
        })?;
        let arc = Arc::new(tracker);
        let _ = self.buddy.set(Arc::clone(&arc));
        Ok(arc)
    }

    /// LLM client (env-driven base URL + API key).
    pub fn llm(&self) -> Arc<LlmClient> {
        self.llm
            .get_or_init(|| Arc::new(LlmClient::new()))
            .clone()
    }

    /// Embedding client (env-driven base URL + API key).
    pub fn embeddings(&self) -> Arc<EmbeddingClient> {
        self.emb
            .get_or_init(|| Arc::new(EmbeddingClient::new()))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_resolves_paths_without_io() {
        // Just construct — no subsystem opens yet.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let ctx = CliContext::new().unwrap();
        assert_eq!(ctx.home(), &dir.path().to_path_buf());
        assert!(ctx.data_dir().ends_with("data"));
        std::env::remove_var("MAKAKOO_HOME");
    }

    #[test]
    fn context_store_opens_lazily() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let ctx = CliContext::new().unwrap();
        let s1 = ctx.store().unwrap();
        let s2 = ctx.store().unwrap();
        assert!(Arc::ptr_eq(&s1, &s2));
        std::env::remove_var("MAKAKOO_HOME");
    }
}
