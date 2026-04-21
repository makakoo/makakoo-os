//! Shared verdict data structures for adapter calls.
//!
//! Mirrors lope's `PhaseVerdict` + `ValidatorResult` so the JSON emitted
//! by `makakoo adapter call <name>` round-trips cleanly into lope's
//! Python dataclasses on the consumer side.

use serde::{Deserialize, Serialize};

/// Verdict status — matches lope's `VerdictStatus` enum exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerdictStatus {
    Pass,
    NeedsFix,
    Fail,
    InfraError,
}

impl VerdictStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictStatus::Pass => "PASS",
            VerdictStatus::NeedsFix => "NEEDS_FIX",
            VerdictStatus::Fail => "FAIL",
            VerdictStatus::InfraError => "INFRA_ERROR",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PASS" => Some(VerdictStatus::Pass),
            "NEEDS_FIX" | "NEEDSFIX" => Some(VerdictStatus::NeedsFix),
            "FAIL" => Some(VerdictStatus::Fail),
            "INFRA_ERROR" | "INFRAERROR" => Some(VerdictStatus::InfraError),
            _ => None,
        }
    }
}

/// One validator's read on a phase. Field names match lope's
/// `PhaseVerdict` dataclass so the Python consumer can hydrate via
/// `PhaseVerdict(**json["verdict"])` with zero translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseVerdict {
    pub status: VerdictStatus,
    pub confidence: f64,
    pub rationale: String,
    #[serde(default)]
    pub required_fixes: Vec<String>,
    #[serde(default)]
    pub nice_to_have: Vec<String>,
    #[serde(default)]
    pub duration_seconds: f64,
    #[serde(default)]
    pub validator_name: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub evidence_gate_triggered: bool,
}

impl PhaseVerdict {
    pub fn infra_error(validator: &str, reason: impl Into<String>) -> Self {
        Self {
            status: VerdictStatus::InfraError,
            confidence: 0.0,
            rationale: reason.into(),
            required_fixes: Vec::new(),
            nice_to_have: Vec::new(),
            duration_seconds: 0.0,
            validator_name: validator.to_string(),
            stage: None,
            evidence_gate_triggered: false,
        }
    }

    pub fn heuristic_pass(validator: &str, rationale: impl Into<String>) -> Self {
        Self {
            status: VerdictStatus::Pass,
            confidence: 0.5,
            rationale: rationale.into(),
            required_fixes: Vec::new(),
            nice_to_have: Vec::new(),
            duration_seconds: 0.0,
            validator_name: validator.to_string(),
            stage: None,
            evidence_gate_triggered: false,
        }
    }
}

/// Top-level payload written to stdout by `makakoo adapter call <name>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorResult {
    pub validator_name: String,
    pub verdict: PhaseVerdict,
    #[serde(default)]
    pub raw_response: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub flag_error_hint: String,
}

impl ValidatorResult {
    pub fn infra_error(validator: &str, err: impl Into<String>) -> Self {
        let e = err.into();
        Self {
            validator_name: validator.to_string(),
            verdict: PhaseVerdict::infra_error(validator, e.clone()),
            raw_response: String::new(),
            error: e,
            flag_error_hint: String::new(),
        }
    }

    /// Convenience: ok if the underlying verdict is anything but INFRA_ERROR
    /// AND no subprocess-level error was recorded.
    pub fn ok(&self) -> bool {
        self.error.is_empty() && self.verdict.status != VerdictStatus::InfraError
    }
}
