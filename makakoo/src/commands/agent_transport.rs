//! CLI side of the supervisor-hosted chat transports.
//!
//! Two entry points, both thin wrappers over
//! `makakoo_core::agents::transport_bridge`:
//!
//!   * [`preflight`] runs in the operator's terminal during `agent
//!     start`, before the service manager takes over. `agent start`
//!     returns once status.json appears, so a problem reported only by
//!     the detached supervisor would never be seen.
//!   * [`plan_bridge`] runs inside `agent _supervisor` and produces the
//!     bridge the supervisor hosts.

use makakoo_core::agents::transport_bridge::{BridgePlan, TransportBridge};
use makakoo_core::agents::AgentSlot;
use makakoo_core::transport::secrets::KeyringSecrets;

use crate::output;

/// Resolve the slot's transports into a runnable bridge.
///
/// Secret resolution is env → keyring → inline. Under launchd the
/// plist sources `~/.env` with `set -a`, so a token written there
/// arrives as a process env var and wins before the keyring is
/// touched — which matters because a headless service cannot answer a
/// keyring prompt.
pub fn plan_bridge(slot: &AgentSlot) -> anyhow::Result<BridgePlan> {
    TransportBridge::plan(slot, &KeyringSecrets).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Fail `agent start` on a transport problem the operator must fix,
/// and warn about the ones the supervisor will work around.
pub fn preflight(slot: &AgentSlot) -> anyhow::Result<()> {
    let plan = plan_bridge(slot)?;
    for skipped in &plan.skipped {
        output::print_warn(format!(
            "transport '{}' will not start — {}",
            skipped.transport_id, skipped.reason
        ));
    }
    let Some(bridge) = plan.bridge else {
        return Ok(());
    };
    let checks = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(bridge.check_credentials())
    });
    let mut rejected = Vec::new();
    for check in checks {
        if check.is_rejected() {
            rejected.push(check.describe());
        } else if !matches!(
            check,
            makakoo_core::agents::transport_bridge::CredentialCheck::Ok { .. }
        ) {
            // A network blip is not a reason to refuse to start: the
            // poll loop retries, and the runtime API works regardless.
            output::print_warn(check.describe());
        }
    }
    if !rejected.is_empty() {
        anyhow::bail!(
            "{}\ncheck the bot token in the transport's secret_env / secret_ref",
            rejected.join("\n")
        );
    }
    Ok(())
}
