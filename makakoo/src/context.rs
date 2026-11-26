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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use once_cell::sync::OnceCell;

use makakoo_core::chat::ChatStore;
use makakoo_core::embeddings::EmbeddingClient;
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::nursery::{BuddyTracker, MascotRegistry};
use makakoo_core::platform::makakoo_home;
use makakoo_core::sancho::SanchoContext;
use makakoo_core::superbrain::folder_index::FolderIndexManager;
use makakoo_core::superbrain::graph::GraphStore;
use makakoo_core::superbrain::memory_stack::MemoryStack;
use makakoo_core::superbrain::promoter::MemoryPromoter;
use makakoo_core::superbrain::store::SuperbrainStore;
use makakoo_core::superbrain::vector_model::{
    check_and_probe_brain_on_open, BrainRebuildCoordinator,
};

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
    bulk_emb: OnceCell<Arc<EmbeddingClient>>,
    folder_index: OnceCell<Arc<FolderIndexManager>>,
    brain_rebuild: Arc<BrainRebuildCoordinator>,
    brain_probed: AtomicBool,
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
            bulk_emb: OnceCell::new(),
            folder_index: OnceCell::new(),
            brain_rebuild: Arc::new(BrainRebuildCoordinator::new()),
            brain_probed: AtomicBool::new(false),
        })
    }

    /// Canonical Makakoo home.
    pub fn home(&self) -> &PathBuf {
        &self.home
    }

    /// Test-only constructor: build a context rooted at `home` without
    /// resolving $MAKAKOO_HOME. Used by unit tests that want a tempdir-
    /// scoped context without touching the real install.
    #[cfg(test)]
    pub fn for_home(home: PathBuf) -> Self {
        let data_dir = home.join("data");
        Self {
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
            bulk_emb: OnceCell::new(),
            folder_index: OnceCell::new(),
            brain_rebuild: Arc::new(BrainRebuildCoordinator::new()),
            brain_probed: AtomicBool::new(false),
        }
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
            format!(
                "failed to open superbrain store at {}",
                self.db_path.display()
            )
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
        let chat = ChatStore::open(&self.chat_path).with_context(|| {
            format!("failed to open chat store at {}", self.chat_path.display())
        })?;
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
            format!(
                "failed to load buddy tracker at {}",
                self.buddy_path.display()
            )
        })?;
        let arc = Arc::new(tracker);
        let _ = self.buddy.set(Arc::clone(&arc));
        Ok(arc)
    }

    /// LLM client (env-driven base URL + API key).
    pub fn llm(&self) -> Arc<LlmClient> {
        self.llm.get_or_init(|| Arc::new(LlmClient::new())).clone()
    }

    /// Embedding client (short calls; env-driven base URL + API key).
    pub fn embeddings(&self) -> Arc<EmbeddingClient> {
        self.emb
            .get_or_init(|| Arc::new(EmbeddingClient::new()))
            .clone()
    }

    /// Bulk embedding client (`MAKAKOO_BULK_EMBED_MODEL` with fallback to
    /// `MAKAKOO_EMBED_MODEL`).
    pub fn bulk_embeddings(&self) -> Arc<EmbeddingClient> {
        self.bulk_emb
            .get_or_init(|| Arc::new(EmbeddingClient::new_bulk()))
            .clone()
    }

    /// Per-folder compact-vector manager used by CLI folder-index status and
    /// explicit rebuild commands. The directory matches the MCP default and
    /// honors the same environment override.
    pub fn folder_index(&self) -> Arc<FolderIndexManager> {
        self.folder_index
            .get_or_init(|| {
                let dir = std::env::var("MAKAKOO_FOLDER_INDEX_DIR")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.data_dir.join("folder-indexes"));
                Arc::new(
                    FolderIndexManager::new(dir)
                        .with_embedders(self.embeddings(), self.bulk_embeddings()),
                )
            })
            .clone()
    }

    #[cfg(test)]
    pub fn with_folder_index_for_test(self, manager: Arc<FolderIndexManager>) -> Self {
        let _ = self.folder_index.set(manager);
        self
    }

    /// The single process-wide Brain vector rebuild coordinator shared by the
    /// store-open probe and SANCHO contexts in this process.
    pub fn brain_rebuild(&self) -> Arc<BrainRebuildCoordinator> {
        Arc::clone(&self.brain_rebuild)
    }

    /// Open the Superbrain store and run the non-blocking alias/endpoint probe
    /// once per process against this context's shared [`BrainRebuildCoordinator`].
    pub fn probe_brain_on_open(&self) -> anyhow::Result<Arc<SuperbrainStore>> {
        let store = self.store()?;
        if !self.brain_probed.swap(true, Ordering::SeqCst) {
            check_and_probe_brain_on_open(
                Arc::clone(&store),
                Some(self.embeddings()),
                self.bulk_embeddings(),
                self.brain_rebuild(),
            );
        }
        Ok(store)
    }

    /// Construct a [`SanchoContext`] wired to this process's shared
    /// [`BrainRebuildCoordinator`] and embedding clients.
    pub fn build_sancho_context(&self) -> anyhow::Result<Arc<SanchoContext>> {
        let store = self.store()?;
        let bus = self.event_bus()?;
        let llm = self.llm();
        let emb = self.embeddings();
        let bulk_emb = self.bulk_embeddings();
        Ok(Arc::new(
            SanchoContext::new(store, bus, llm, emb, self.home.clone())
                .with_bulk_embeddings(bulk_emb)
                .with_brain_rebuild(self.brain_rebuild()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_resolves_paths_without_io() {
        // Just construct — no subsystem opens yet. Takes the crate-wide
        // ENV_MUTEX so we don't race another test also setting MAKAKOO_HOME.
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let ctx = CliContext::new().unwrap();
        assert_eq!(ctx.home(), &dir.path().to_path_buf());
        assert!(ctx.data_dir().ends_with("data"));
        std::env::remove_var("MAKAKOO_HOME");
    }

    #[test]
    fn context_store_opens_lazily() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
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
