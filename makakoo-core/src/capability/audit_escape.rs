//! Backward-compat shim — actual implementation lives in
//! `garagetytus_grants::audit_escape` (carved 2026-04-25 per
//! GARAGETYTUS-V0.1 Phase A.5 / A.1).
//!
//! Mirrors Python `core.capability.user_grants.escape_audit_field`;
//! both sides read the same `audit_escape_vectors.json` fixture
//! (vendored into the garagetytus repo for the Rust drift gate).

pub use garagetytus_grants::audit_escape::*;
