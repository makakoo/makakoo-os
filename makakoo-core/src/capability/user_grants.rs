//! Backward-compat shim — actual implementation lives in the
//! standalone `garagetytus-grants` crate (carved 2026-04-25 per
//! GARAGETYTUS-V0.1 Phase A.5).
//!
//! Every consumer's import path stays valid:
//! `crate::capability::user_grants::UserGrants`,
//! `crate::capability::user_grants::default_path`, etc. — they all
//! resolve through this re-export to the carved-out crate.
//!
//! Schema (version 1) is frozen across both repos. Mutating the
//! schema requires coordinated updates here AND in `garagetytus-sdk`
//! (Python mirror, Phase C). See `spec/USER_GRANTS.md §3` in
//! makakoo-os for the locked field set.

pub use garagetytus_grants::*;
