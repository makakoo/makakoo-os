//! GYM Layer 1 — error capture funnel.
//!
//! Three independent producers (bash wrapper, Claude Code Stop hook,
//! Python `@log_errors` decorator, SANCHO task supervisor, manual
//! `makakoo flag` CLI) all funnel into the same on-disk JSONL schema:
//!
//! `data/errors/YYYY-MM-DD/<source>.jsonl`
//!
//! Layers 2-5 (classifier → hypothesis → lope_gate → approval) live in
//! the existing `mascot-gym` plugin. This module only owns the hot-path
//! capture write so failed kernel paths (SANCHO tasks, plugin spawn
//! failures, capability denials) can record without paying GIL or
//! Python startup cost.

pub mod capture;

pub use capture::{ErrorCapture, ErrorEntry, ErrorSource};
