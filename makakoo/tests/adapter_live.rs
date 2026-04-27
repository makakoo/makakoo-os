//! v0.6 Phase A — live dogfood integration tests for the 3 new bundled
//! adapters (`pi`, `tytus-cli`, `switchailocal`). Gated on
//! `MAKAKOO_ADAPTER_LIVE=1` because each test actually reaches out over
//! the real wire (network or subprocess) and fails on a stock CI host
//! where those services aren't running.
//!
//! What each test proves:
//!   - pi:            `pi` binary on PATH, responds to a stdin prompt.
//!   - tytus-cli:     `tytus-mcp` on PATH, accepts MCP `tools/call` with
//!                    a fan-out envelope `{"tool":"tytus_status"}`.
//!   - switchailocal: a SwitchAILocal gateway is listening on
//!                    127.0.0.1:18080 with `AIL_API_KEY` in the env.
//!
//! Non-goals: verdict semantics, streaming, model choice. The bar is
//! "the bridge successfully carried a prompt to the target and brought
//! back a real response of the expected shape". Content assertions stay
//! loose so transient upstream quirks (model swap, updated wording)
//! don't break tests.
//!
//! To run:
//!     MAKAKOO_ADAPTER_LIVE=1 cargo test --test adapter_live
//!
//! Discoverability note: lope-style regression tests that want to
//! exercise the same adapters against real services can follow this
//! exact shape — load the bundled manifest via `Manifest::load`, spawn
//! a `CallContext`, then call `call_adapter` and assert on the
//! `ValidatorResult`.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use makakoo_core::adapter::{call_adapter_with_default_timeout, Manifest};

fn bundled(name: &str) -> Manifest {
    let path = workspace_root()
        .join("plugins-core/adapters")
        .join(name)
        .join("adapter.toml");
    Manifest::load(&path).unwrap_or_else(|e| panic!("load {name}: {e}"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn gated() -> bool {
    std::env::var("MAKAKOO_ADAPTER_LIVE").ok().as_deref() == Some("1")
}

#[tokio::test]
async fn pi_adapter_returns_real_response() {
    if !gated() {
        eprintln!("skipped (set MAKAKOO_ADAPTER_LIVE=1 to run)");
        return;
    }
    let manifest = bundled("pi");
    let result = call_adapter_with_default_timeout(
        &manifest,
        "Reply with just the digit 4 and nothing else.",
        Duration::from_secs(90),
    )
    .await;
    assert_eq!(result.error, "", "pi returned error: {}", result.error);
    // pi should have said something — not necessarily "4" (model may
    // over-explain). Just assert the raw response is non-empty.
    assert!(
        !result.raw_response.trim().is_empty(),
        "pi raw_response empty"
    );
}

#[tokio::test]
async fn tytus_cli_mcp_envelope_round_trip() {
    if !gated() {
        eprintln!("skipped (set MAKAKOO_ADAPTER_LIVE=1 to run)");
        return;
    }
    let manifest = bundled("tytus-cli");
    // Pick the cheapest always-available tool — tytus_status — which
    // returns a JSON doc whether the user is logged in or not.
    let envelope = r#"{"tool":"tytus_status","arguments":{}}"#;
    let result = call_adapter_with_default_timeout(
        &manifest,
        envelope,
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(result.error, "", "tytus-cli error: {}", result.error);
    assert!(
        !result.raw_response.trim().is_empty(),
        "tytus-cli returned empty content"
    );
    // Response must look like JSON — tytus_status embeds a JSON doc
    // inside the MCP content.0.text field.
    assert!(
        result.raw_response.trim_start().starts_with('{'),
        "expected JSON-shaped raw_response, got: {}",
        result.raw_response
    );
}

#[tokio::test]
async fn switchailocal_openai_compat_round_trip() {
    if !gated() {
        eprintln!("skipped (set MAKAKOO_ADAPTER_LIVE=1 to run)");
        return;
    }
    if std::env::var("AIL_API_KEY").is_err() {
        eprintln!("skipped (AIL_API_KEY unset)");
        return;
    }
    let manifest = bundled("switchailocal");
    let result = call_adapter_with_default_timeout(
        &manifest,
        "Reply with just the digit 4.",
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(
        result.error, "",
        "switchailocal error: {}",
        result.error
    );
    assert!(
        !result.raw_response.trim().is_empty(),
        "switchailocal returned empty content"
    );
}
