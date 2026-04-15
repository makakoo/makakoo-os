//! Test-only helpers shared across the `makakoo` bin crate.
//!
//! Specifically: a global mutex that any test mutating `MAKAKOO_HOME`
//! (or other process-wide env vars) must hold while it does so.
//!
//! `cargo test` runs tests in parallel by default, and `std::env::set_var`
//! is process-global, so two tests racing on the same env var produce
//! non-deterministic failures. The mutex is cheap and makes the racing
//! tests run sequentially without needing `--test-threads=1`.
//!
//! This module is `#[cfg(test)]` only — no production code ever imports
//! anything from here.

#![cfg(test)]

use std::sync::Mutex;

/// Crate-wide lock for tests that mutate `MAKAKOO_HOME` / `HARVEY_HOME`
/// / other process env vars. Every such test should
/// `let _guard = ENV_MUTEX.lock().unwrap();` at the top of its body.
pub static ENV_MUTEX: Mutex<()> = Mutex::new(());
