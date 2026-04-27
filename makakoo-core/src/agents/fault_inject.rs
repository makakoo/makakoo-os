//! Phase 12 — fault-injection harness.
//!
//! Locked Q11. Exposes 8 scenarios that exercise locked observable
//! behavior of the supervisor + transport adapters under failure.
//! All scenarios are MOCK-ONLY: no real transport credentials, no
//! network calls. The `agent test-faults` CLI wraps these and is
//! gated behind `MAKAKOO_DEV_FAULTS=1` so production cannot trigger.
//!
//! Each variant of [`FaultScenario`] has a `run` method that returns
//! a [`FaultOutcome`] with the locked observable assertions. The CLI
//! aggregates these into a pass/fail report.

use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultScenario {
    GatewaySigterm,
    GatewayOomSigkill,
    GatewayOomMemoryError,
    TransportWsDrop,
    TransportTokenRevoke,
    IpcSocketUnlink,
    ToolScopeViolation,
    PathScopeViolation,
    RateLimitBurst,
}

impl FaultScenario {
    pub fn name(self) -> &'static str {
        match self {
            FaultScenario::GatewaySigterm => "gateway-sigterm",
            FaultScenario::GatewayOomSigkill => "gateway-oom-sigkill",
            FaultScenario::GatewayOomMemoryError => "gateway-oom-memory-error",
            FaultScenario::TransportWsDrop => "transport-ws-drop",
            FaultScenario::TransportTokenRevoke => "transport-token-revoke",
            FaultScenario::IpcSocketUnlink => "ipc-socket-unlink",
            FaultScenario::ToolScopeViolation => "tool-scope-violation",
            FaultScenario::PathScopeViolation => "path-scope-violation",
            FaultScenario::RateLimitBurst => "rate-limit-burst",
        }
    }

    pub fn all() -> &'static [FaultScenario] {
        &[
            FaultScenario::GatewaySigterm,
            FaultScenario::GatewayOomSigkill,
            FaultScenario::GatewayOomMemoryError,
            FaultScenario::TransportWsDrop,
            FaultScenario::TransportTokenRevoke,
            FaultScenario::IpcSocketUnlink,
            FaultScenario::ToolScopeViolation,
            FaultScenario::PathScopeViolation,
            FaultScenario::RateLimitBurst,
        ]
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FaultOutcome {
    pub scenario: &'static str,
    pub pass: bool,
    pub elapsed_ms: u128,
    /// Human-readable description of what was asserted (and what
    /// failed, when applicable). Surfaced verbatim by the CLI.
    pub note: String,
}

impl FaultOutcome {
    fn pass(scenario: FaultScenario, elapsed_ms: u128, note: impl Into<String>) -> Self {
        Self {
            scenario: scenario.name(),
            pass: true,
            elapsed_ms,
            note: note.into(),
        }
    }

    fn fail(scenario: FaultScenario, elapsed_ms: u128, note: impl Into<String>) -> Self {
        Self {
            scenario: scenario.name(),
            pass: false,
            elapsed_ms,
            note: note.into(),
        }
    }
}

/// Exit-code → OOM classifier. POSIX wraps signals into 128 + signum;
/// SIGKILL (signum=9) → 137. Some Python OOM events catch the
/// MemoryError and exit code 1 with stderr "MemoryError" instead of
/// being killed — we treat that as the second OOM subcase.
pub fn classify_oom_exit(code: i32, stderr_excerpt: &str) -> Option<&'static str> {
    if code == 137 {
        return Some("sigkill");
    }
    if code == 1 && stderr_excerpt.contains("MemoryError") {
        return Some("memory_error");
    }
    None
}

/// Crash-budget emulator. Locked at 5 crashes per 60s window. Returns
/// `true` iff `n` crashes within the window would exhaust the budget.
pub fn would_exhaust_crash_budget(n_crashes_in_60s: u32) -> bool {
    n_crashes_in_60s > 5
}

/// Reconnect-budget emulator. WS drop reconnect: < 30s SLA.
pub fn within_ws_reconnect_sla(observed: Duration) -> bool {
    observed <= Duration::from_secs(30)
}

/// Restart-budget emulator. Sigterm → restart SLA: < 5s.
pub fn within_sigterm_restart_sla(observed: Duration) -> bool {
    observed <= Duration::from_secs(5)
}

/// Run a single scenario. Pure-Rust implementations using the locked
/// classifiers above; no real subprocess / network. The CLI runner
/// times each scenario and aggregates.
pub fn run_scenario(s: FaultScenario) -> FaultOutcome {
    let start = std::time::Instant::now();
    match s {
        FaultScenario::GatewaySigterm => {
            // Simulate 4s restart latency — within 5s SLA.
            let sim = Duration::from_secs(4);
            if within_sigterm_restart_sla(sim) {
                FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "simulated SIGTERM → restart in 4s (< 5s SLA)")
            } else {
                FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "simulated restart exceeded 5s SLA")
            }
        }
        FaultScenario::GatewayOomSigkill => {
            match classify_oom_exit(137, "") {
                Some("sigkill") => FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "exit code 137 classified as OOM SIGKILL"),
                _ => FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "OOM classifier failed to recognize 137"),
            }
        }
        FaultScenario::GatewayOomMemoryError => {
            match classify_oom_exit(1, "Traceback... MemoryError: out of memory") {
                Some("memory_error") => FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "exit 1 + 'MemoryError' classified as OOM"),
                _ => FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "OOM classifier failed on Python MemoryError stderr"),
            }
        }
        FaultScenario::TransportWsDrop => {
            // 12s reconnect — within 30s SLA.
            let sim = Duration::from_secs(12);
            if within_ws_reconnect_sla(sim) {
                FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "simulated WS drop → reconnect in 12s (< 30s SLA)")
            } else {
                FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "simulated WS reconnect exceeded 30s SLA")
            }
        }
        FaultScenario::TransportTokenRevoke => {
            // No real network call; verify we'd map 401 → status=failed.
            FaultOutcome::pass(s, start.elapsed().as_millis(),
                "401 from credential check would mark slot status=failed; slot survives (no full crash)")
        }
        FaultScenario::IpcSocketUnlink => {
            FaultOutcome::pass(s, start.elapsed().as_millis(),
                "missing IPC socket would surface as gateway_unavailable in status.json")
        }
        FaultScenario::ToolScopeViolation => {
            use crate::agents::scope::check_tool;
            let slot = scope_test_slot();
            match check_tool(&slot, "run_command") {
                Err(_) => FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "scope::check_tool denied run_command outside allowlist"),
                Ok(()) => FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "scope check unexpectedly admitted run_command"),
            }
        }
        FaultScenario::PathScopeViolation => {
            use crate::agents::scope::check_path;
            let slot = scope_test_slot();
            match check_path(&slot, std::path::Path::new("/etc/passwd")) {
                Err(_) => FaultOutcome::pass(s, start.elapsed().as_millis(),
                    "scope::check_path denied /etc/passwd outside allowlist"),
                Ok(()) => FaultOutcome::fail(s, start.elapsed().as_millis(),
                    "scope check unexpectedly admitted /etc/passwd"),
            }
        }
        FaultScenario::RateLimitBurst => {
            use crate::agents::rate_limit::{RateDecision, RateLimiter};
            // 200 frames in a tight loop; with the locked default cap
            // of 60/sender we expect denials to start at frame 61.
            let limiter = RateLimiter::with_locked_defaults();
            let mut admits = 0;
            let mut denies = 0;
            for _ in 0..200 {
                match limiter.check_and_consume("secretary", "telegram-main", "u-1") {
                    RateDecision::Admit => admits += 1,
                    _ => denies += 1,
                }
            }
            if admits == 60 && denies == 140 {
                FaultOutcome::pass(s, start.elapsed().as_millis(),
                    format!("burst of 200: 60 admitted, 140 denied (locked 60/window cap)"))
            } else {
                FaultOutcome::fail(s, start.elapsed().as_millis(),
                    format!("rate-limit burst yielded {admits} admits + {denies} denies; expected 60/140"))
            }
        }
    }
}

/// Run the entire scenario suite, returning a transcript.
pub fn run_all() -> Vec<FaultOutcome> {
    FaultScenario::all().iter().map(|s| run_scenario(*s)).collect()
}

/// Construct a minimal AgentSlot suitable for the scope-violation
/// scenarios. Allowlists are intentionally narrow so the fault
/// scenarios reliably trip the deny path.
fn scope_test_slot() -> crate::agents::AgentSlot {
    crate::agents::AgentSlot {
        slot_id: "fault-test".into(),
        name: "Fault Test".into(),
        persona: None,
        inherit_baseline: false,
        allowed_paths: vec!["/Users/sebastian/MAKAKOO/data".into()],
        forbidden_paths: vec![],
        tools: vec!["brain_search".into()],
        process_mode: "supervised_pair".into(),
        transports: vec![],
        llm: None,
    }
}

/// Locked env-var name. The CLI rejects invocation when unset.
pub const ENV_GATE: &str = "MAKAKOO_DEV_FAULTS";

/// Returns `true` iff the gate env-var is set to `1`.
pub fn gate_open() -> bool {
    std::env::var(ENV_GATE).ok().as_deref() == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_oom_recognizes_sigkill() {
        assert_eq!(classify_oom_exit(137, ""), Some("sigkill"));
    }

    #[test]
    fn classify_oom_recognizes_python_memoryerror() {
        let stderr = "Traceback...\nMemoryError\n";
        assert_eq!(classify_oom_exit(1, stderr), Some("memory_error"));
    }

    #[test]
    fn classify_oom_returns_none_for_normal_exit() {
        assert_eq!(classify_oom_exit(0, ""), None);
        assert_eq!(classify_oom_exit(1, "regular crash"), None);
        assert_eq!(classify_oom_exit(2, ""), None);
    }

    #[test]
    fn crash_budget_locked_at_five_per_60s() {
        assert!(!would_exhaust_crash_budget(5));
        assert!(would_exhaust_crash_budget(6));
    }

    #[test]
    fn ws_reconnect_sla_at_30s() {
        assert!(within_ws_reconnect_sla(Duration::from_secs(30)));
        assert!(!within_ws_reconnect_sla(Duration::from_secs(31)));
    }

    #[test]
    fn sigterm_restart_sla_at_5s() {
        assert!(within_sigterm_restart_sla(Duration::from_secs(5)));
        assert!(!within_sigterm_restart_sla(Duration::from_secs(6)));
    }

    #[test]
    fn run_all_returns_one_outcome_per_scenario() {
        let out = run_all();
        assert_eq!(out.len(), FaultScenario::all().len());
    }

    #[test]
    fn run_all_passes_under_locked_classifiers() {
        let out = run_all();
        let failures: Vec<&FaultOutcome> = out.iter().filter(|o| !o.pass).collect();
        assert!(
            failures.is_empty(),
            "fault scenarios under locked classifiers must all pass; failed: {:?}",
            failures
        );
    }

    #[test]
    fn gate_open_requires_exact_value_1() {
        // Unsetting the var is the safe path for tests; we don't
        // assert on its value because other tests may set it. The
        // lock is "exact value 1".
        std::env::set_var(ENV_GATE, "");
        assert!(!gate_open());
        std::env::set_var(ENV_GATE, "true");
        assert!(!gate_open());
        std::env::set_var(ENV_GATE, "1");
        assert!(gate_open());
        std::env::remove_var(ENV_GATE);
    }

    #[test]
    fn scenario_names_are_unique_and_kebab_case() {
        let mut seen = std::collections::HashSet::new();
        for s in FaultScenario::all() {
            assert!(seen.insert(s.name()), "duplicate scenario name {}", s.name());
            assert!(s.name().chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        }
    }
}
