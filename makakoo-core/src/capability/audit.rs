//! Backward-compat shim — actual implementation lives in
//! `garagetytus_grants::audit` (carved 2026-04-25 per
//! GARAGETYTUS-V0.1 Phase A.5 / A.1).
//!
//! Append-only audit log; `AuditEntry` schema is version 1 and frozen
//! across both repos.

pub use garagetytus_grants::audit::*;
