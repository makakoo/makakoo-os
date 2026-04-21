//! Telemetry subsystem — cost tracking and (later) feature flags.
//!
//! Wave 3 scope (T11): `costs::CostTracker`, the Rust port of
//! `core/telemetry/cost_tracker.py`. Python writes to a JSONL file;
//! the Rust rewrite is authoritative on sqlite.

pub mod aggregate;
pub mod costs;
pub mod tracing_setup;
pub use aggregate::{audit_rollup, rollup_default, AuditRollup, Period, RollupRow};
pub use costs::{CostRecord, CostSummary, CostTracker};
pub use tracing_setup::{init_stderr, LogFormat};
