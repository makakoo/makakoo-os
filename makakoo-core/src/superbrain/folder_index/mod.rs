//! Per-folder compact vector + place index for knowledge folders.
//!
//! Each knowledge folder lives in its own directory `<base_dir>/<folder_id>/`
//! containing:
//!
//! - `chunks.sqlite` — source of truth for chunk text, exact source places,
//!   provenance filter columns (`source_kind`, `trust`, `owner`), file
//!   states (`indexed`, `indexing`, `skipped`), generation counter, and
//!   compact `i8` vectors (up to 512 dims = 512 bytes/chunk).
//! - `vectors.usearch` — memory-mapped (`usearch`, `ScalarKind::I8`,
//!   `MetricKind::Cos`) HNSW nearest-neighbour index.
//! - `vectors.gen` — atomic sidecar recording the generation built into
//!   `vectors.usearch`. If `vectors.usearch` is missing or behind SQLite's
//!   generation on open, it is rebuilt deterministically from SQLite without
//!   re-embedding.
//!
//! Existing [`super::store::SuperbrainStore`] behaviour is untouched.

pub mod chunker;
pub mod fts;
pub mod manager;
pub mod model_meta;
pub mod quantize;
pub mod rebuild;
mod rebuild_pages;
mod sqlite_repo;
pub mod store;
mod vector_store;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod model_open_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod tests;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::embeddings::{resolve_bulk_model, resolve_short_model};
use crate::error::{MakakooError, Result};

pub use chunker::{
    anchor_from_segment, chunk_file_engine_output, chunk_segment, ChunkOptions,
    FileEngineExtractor, FileEngineOutput, FileEngineSegment, TextChunk,
};
pub use manager::{FolderIndexManager, FolderRebuildRequestOutcome};
pub use model_meta::{
    FolderModelStatus, FolderSearchRequest, FolderSearchResponse, IndexModelState, SearchMode,
    UpsertModelMeta,
};
pub use quantize::{
    bytes_to_i8_vec, cosine_similarity_i8, distance_to_cosine_score, i8_slice_to_bytes,
    truncate_and_quantize_i8,
};
pub use store::FolderIndex;

/// Hard upper bound on indexed vector dimensionality.
pub const MAX_VECTOR_DIM: usize = 512;
/// Default indexed vector dimensionality (truncated from larger embeddings).
pub const DEFAULT_VECTOR_DIM: usize = 512;

/// Default chunker bounds (Unicode code points).
pub const DEFAULT_CHUNK_MIN_CHARS: usize = 800;
pub const DEFAULT_CHUNK_TARGET_CHARS: usize = 1000;
pub const DEFAULT_CHUNK_MAX_CHARS: usize = 1200;
pub const DEFAULT_CHUNK_OVERLAP_CHARS: usize = 100;

/// Default batched flush thresholds during indexing (no per-file rewrites).
pub const DEFAULT_FLUSH_FILE_BATCH: usize = 500;
pub const DEFAULT_FLUSH_INTERVAL_SECS: u64 = 30;

/// Default HNSW graph parameters tuned for ~100–130 MB vector index at 200k
/// 512-dim i8 chunks, < 0.5 GB peak RSS while indexing, and sub-10ms p95 search.
pub const DEFAULT_HNSW_CONNECTIVITY: usize = 12;
pub const DEFAULT_HNSW_EXPANSION_ADD: usize = 64;
pub const DEFAULT_HNSW_EXPANSION_SEARCH: usize = 64;

/// Anchor kind inside a file. Matches the citation wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorKind {
    Page,
    Sheet,
    Slide,
    Heading,
}

impl AnchorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Sheet => "sheet",
            Self::Slide => "slide",
            Self::Heading => "heading",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "page" => Ok(Self::Page),
            "sheet" => Ok(Self::Sheet),
            "slide" => Ok(Self::Slide),
            "heading" => Ok(Self::Heading),
            other => Err(MakakooError::invalid_input(format!(
                "invalid anchor kind: {other:?}"
            ))),
        }
    }
}

/// Exact structural location of a passage inside its source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub kind: AnchorKind,
    pub value: String,
}

/// Provenance source category for a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    File,
    Memory,
    LaptopNote,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Memory => "memory",
            Self::LaptopNote => "laptop_note",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "file" => Ok(Self::File),
            "memory" => Ok(Self::Memory),
            "laptop_note" => Ok(Self::LaptopNote),
            other => Err(MakakooError::invalid_input(format!(
                "invalid source_kind: {other:?}"
            ))),
        }
    }
}

/// Trust classification for a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Untrusted => "untrusted",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "trusted" => Ok(Self::Trusted),
            "untrusted" => Ok(Self::Untrusted),
            other => Err(MakakooError::invalid_input(format!(
                "invalid trust level: {other:?}"
            ))),
        }
    }
}

/// Honest reason why a file could not be indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Password,
    TooBig,
    Unreadable,
}

impl SkipReason {
    /// Canonical list of every skip reason in wire order (single source of truth).
    pub const ALL: [Self; 3] = [Self::Password, Self::TooBig, Self::Unreadable];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::TooBig => "too_big",
            Self::Unreadable => "unreadable",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == raw)
            .ok_or_else(|| MakakooError::invalid_input(format!("invalid skip reason: {raw:?}")))
    }
}

/// Honest per-file indexing state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileStatus {
    Indexed,
    Indexing { done: u64, total: u64 },
    Skipped { reason: SkipReason },
}

/// Per-file state record reported to callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStateRecord {
    pub file_path: String,
    pub file_version: String,
    #[serde(flatten)]
    pub status: FileStatus,
}

/// Passage returned by folder search.
///
/// Wire contract is locked to these exact 10 keys; `owner` is filter-only
/// in SQLite and is intentionally excluded from the passage JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Passage {
    pub folder_id: String,
    pub file_path: String,
    pub file_version: String,
    pub anchor: Anchor,
    pub char_start: usize,
    pub char_end: usize,
    pub text: String,
    pub score: f32,
    pub source_kind: SourceKind,
    pub trust: TrustLevel,
}

/// Provenance filter predicate applied during folder search.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilter {
    #[serde(default)]
    pub source_kind: Option<SourceKind>,
    #[serde(default)]
    pub trust: Option<TrustLevel>,
    #[serde(default)]
    pub owner: Option<String>,
}

impl SearchFilter {
    pub fn is_empty(&self) -> bool {
        self.source_kind.is_none() && self.trust.is_none() && self.owner.is_none()
    }
}

/// One chunk + embedding vector to write for a file.
#[derive(Debug, Clone)]
pub struct ChunkInput {
    pub anchor: Anchor,
    pub char_start: usize,
    pub char_end: usize,
    pub text: String,
    pub vector: Vec<f32>,
}

/// Request to add or replace a file's chunks in a folder index.
#[derive(Debug, Clone)]
pub struct UpsertFileRequest {
    pub file_path: String,
    pub file_version: String,
    pub source_kind: SourceKind,
    pub trust: TrustLevel,
    pub owner: String,
    pub chunks: Vec<ChunkInput>,
}

/// Outcome of an `upsert_file` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertOutcome {
    pub replaced: bool,
    pub skipped_unchanged: bool,
    pub chunks_written: usize,
}

/// On-disk size breakdown for a folder index, reporting the vector index
/// and the SQLite source of truth separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSizeReport {
    pub vector_index_bytes: u64,
    pub sqlite_bytes: u64,
    pub chunk_count: u64,
    pub generation: u64,
}

/// Configuration knobs for opening a [`FolderIndex`].
#[derive(Debug, Clone)]
pub struct FolderIndexConfig {
    pub dim: usize,
    pub flush_file_batch: usize,
    pub flush_interval: Duration,
    pub connectivity: usize,
    pub expansion_add: usize,
    pub expansion_search: usize,
    pub embed_alias: Option<String>,
    pub short_embed_alias: Option<String>,
}

impl Default for FolderIndexConfig {
    fn default() -> Self {
        Self::with_dim(DEFAULT_VECTOR_DIM)
    }
}

impl FolderIndexConfig {
    pub fn with_dim(dim: usize) -> Self {
        Self {
            dim,
            flush_file_batch: DEFAULT_FLUSH_FILE_BATCH,
            flush_interval: Duration::from_secs(DEFAULT_FLUSH_INTERVAL_SECS),
            connectivity: DEFAULT_HNSW_CONNECTIVITY,
            expansion_add: DEFAULT_HNSW_EXPANSION_ADD,
            expansion_search: DEFAULT_HNSW_EXPANSION_SEARCH,
            embed_alias: Some(resolve_bulk_model()),
            short_embed_alias: Some(resolve_short_model()),
        }
    }

    pub fn with_embed_aliases(
        mut self,
        bulk_alias: Option<String>,
        short_alias: Option<String>,
    ) -> Self {
        self.embed_alias = bulk_alias;
        self.short_embed_alias = short_alias;
        self
    }
}

/// Guard against path traversal and invalid directory names for `folder_id`.
pub fn validate_folder_id(folder_id: &str) -> Result<()> {
    if folder_id.is_empty() || folder_id.len() > 128 {
        return Err(MakakooError::invalid_input(
            "folder_id must be between 1 and 128 characters",
        ));
    }
    if folder_id.starts_with('.') || folder_id.contains("..") {
        return Err(MakakooError::invalid_input(format!(
            "folder_id {folder_id:?} must not start with '.' or contain '..'"
        )));
    }
    if !folder_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err(MakakooError::invalid_input(format!(
            "folder_id {folder_id:?} contains disallowed characters (allowed: [A-Za-z0-9._-])"
        )));
    }
    Ok(())
}

/// Guard against empty or NUL-containing file paths.
pub fn validate_file_path(file_path: &str) -> Result<()> {
    if file_path.trim().is_empty() || file_path.contains('\0') {
        return Err(MakakooError::invalid_input(
            "file_path must be non-empty and contain no NUL bytes",
        ));
    }
    Ok(())
}

/// Guard against out-of-range vector dimensionality.
pub fn validate_dim(dim: usize) -> Result<()> {
    if dim == 0 || dim > MAX_VECTOR_DIM {
        return Err(MakakooError::invalid_input(format!(
            "vector dim {dim} out of range 1..={MAX_VECTOR_DIM}"
        )));
    }
    Ok(())
}
