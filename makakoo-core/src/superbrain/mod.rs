//! Superbrain subsystem — SQLite FTS5 + vector storage.
//!
//! Wave 2 scope: port `core/superbrain/store.py` to Rust while keeping
//! the sqlite schema byte-identical to the Python source of truth. Later
//! phases layer memory stack, recall, promotion, and graph features on
//! top of the primitives exposed here.
//!
//! Every gotcha locked in by the T1 Python acceptance oracle is preserved:
//!
//! * BM25 scores are stored negative by FTS5 and flipped on read
//!   (higher = better).
//! * Journal documents receive a recency boost after the flip.
//! * Cosine similarity returns `0.0` on dim mismatch — never panics, never
//!   errors.
//! * `vector_search` drops any hit below a 0.3 similarity floor.
//! * Vector blobs are little-endian f32, matching Python's
//!   `struct.pack(f"{n}f", ...)` byte layout.
//! * FTS5 tables use `tokenize='porter unicode61'` so `cafe` matches
//!   `café` and `naïve` matches `naive`.

pub mod store;
pub use store::{Document, SearchHit, StoreStats, SuperbrainStore, VectorHit};

// T5 modules — graph, memory_stack, recall, scorer, promoter.
pub mod graph;
pub mod ingest;
pub mod memory_stack;
pub mod promoter;
pub mod recall;
pub mod scorer;
