//! `makakoo agent test-faults` — Phase 12 / Q11 fault scenario runner.
//!
//! Gated behind `MAKAKOO_DEV_FAULTS=1` so production cannot trigger.
//! All scenarios are mock-only — no real transport credentials, no
//! network calls.

use makakoo_core::agents::fault_inject::{
    gate_open, run_all, run_scenario, FaultOutcome, FaultScenario, ENV_GATE,
};

pub fn run(scenario_name: Option<String>, json: bool) -> anyhow::Result<()> {
    if !gate_open() {
        anyhow::bail!(
            "fault-injection runner is gated — set {ENV_GATE}=1 to enable. \
             This guard prevents prod from triggering destructive test scenarios."
        );
    }
    let outcomes: Vec<FaultOutcome> = match scenario_name {
        Some(name) => {
            let s = parse_scenario(&name)?;
            vec![run_scenario(s)]
        }
        None => run_all(),
    };
    if json {
        for o in &outcomes {
            let line = serde_json::to_string(o)
                .map_err(|e| anyhow::anyhow!("serialize outcome: {e}"))?;
            println!("{line}");
        }
    } else {
        render(&outcomes);
    }
    let any_failed = outcomes.iter().any(|o| !o.pass);
    if any_failed {
        std::process::exit(1);
    }
    Ok(())
}

fn parse_scenario(name: &str) -> anyhow::Result<FaultScenario> {
    FaultScenario::all()
        .iter()
        .copied()
        .find(|s| s.name() == name)
        .ok_or_else(|| {
            let known = FaultScenario::all()
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("unknown scenario '{name}' — known: {known}")
        })
}

fn render(outcomes: &[FaultOutcome]) {
    println!(
        "{:<28} {:<5} {:>8}  {}",
        "scenario", "pass", "elapsed", "note"
    );
    println!("{}", "-".repeat(100));
    for o in outcomes {
        let badge = if o.pass { "PASS" } else { "FAIL" };
        println!(
            "{:<28} {:<5} {:>6}ms  {}",
            o.scenario, badge, o.elapsed_ms, o.note
        );
    }
    let pass = outcomes.iter().filter(|o| o.pass).count();
    let fail = outcomes.len() - pass;
    println!("\n{} pass / {} fail", pass, fail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scenario_accepts_kebab_names() {
        let s = parse_scenario("gateway-sigterm").unwrap();
        assert_eq!(s, FaultScenario::GatewaySigterm);
    }

    #[test]
    fn parse_scenario_rejects_unknown() {
        let err = parse_scenario("not-a-thing").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("known:"));
        assert!(msg.contains("gateway-sigterm"));
    }
}
