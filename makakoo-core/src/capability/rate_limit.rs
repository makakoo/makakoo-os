//! Backward-compat shim — actual implementation lives in
//! `garagetytus_grants::rate_limit` (carved 2026-04-25 per
//! GARAGETYTUS-V0.1 Phase A.5 / A.1).
//!
//! Schema-locked across both repos at version 1; the
//! `rate_limit_decrement_vectors.json` drift fixture in
//! `plugins-core/lib-harvey-core/tests/fixtures/` and the carved
//! copy in the garagetytus repo's `tests/fixtures/` must stay
//! identical.

pub use garagetytus_grants::rate_limit::*;
