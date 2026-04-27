//! SANCHO gate primitives — composable precondition checks.
//!
//! Rust port of `core/sancho/gates.py`. Each gate implements [`Gate`] and
//! decides whether a task is allowed to run *right now* given the shared
//! [`GateState`] (per-task last-run timestamps + busy locks) and wall-clock.
//!
//! Gates observe `now` + immutable state and return bool. Mutation of
//! last-run timestamps and locks happens in
//! [`crate::sancho::engine::SanchoEngine::tick_once`].

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, Timelike, Weekday};

/// Precondition check. A task runs only if *all* its gates return `true`.
pub trait Gate: Send + Sync {
    /// Short diagnostic name (appears in logs and status output).
    fn name(&self) -> &str;
    /// Decide whether `task_name` may run at `now` given `state`.
    fn allows(&self, task_name: &str, now: DateTime<Local>, state: &GateState) -> bool;
}

/// Shared state observed by every gate.
#[derive(Default, Debug, Clone)]
pub struct GateState {
    /// Per-task last-run timestamp.
    pub last_run: HashMap<String, DateTime<Local>>,
    /// Per-task busy flag set while the handler is executing.
    pub locks: HashMap<String, bool>,
    /// True when Harvey is actively handling a user query.
    pub session_active: bool,
}

impl GateState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `task_name` as busy. Returns `true` if it was free before.
    pub fn try_acquire(&mut self, task_name: &str) -> bool {
        let busy = self.locks.get(task_name).copied().unwrap_or(false);
        if busy {
            return false;
        }
        self.locks.insert(task_name.to_string(), true);
        true
    }

    /// Release the busy flag. Safe to call even if no lock is held.
    pub fn release(&mut self, task_name: &str) {
        self.locks.insert(task_name.to_string(), false);
    }

    /// Record a successful run at `when`.
    pub fn record_run(&mut self, task_name: &str, when: DateTime<Local>) {
        self.last_run.insert(task_name.to_string(), when);
    }
}

// ─────────────────────────────────────────────────────────────────────
//  Built-in gates
// ─────────────────────────────────────────────────────────────────────

/// Require at least `interval` between runs.
#[derive(Debug, Clone)]
pub struct TimeGate {
    pub interval: Duration,
    name: String,
}

impl TimeGate {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            name: "time".to_string(),
        }
    }
}

impl Gate for TimeGate {
    fn name(&self) -> &str {
        &self.name
    }

    fn allows(&self, task_name: &str, now: DateTime<Local>, state: &GateState) -> bool {
        match state.last_run.get(task_name) {
            None => true,
            Some(prev) => {
                let elapsed = now.signed_duration_since(*prev);
                let elapsed_std = match elapsed.to_std() {
                    Ok(d) => d,
                    Err(_) => Duration::MAX,
                };
                elapsed_std >= self.interval
            }
        }
    }
}

/// Block when a user session is active.
#[derive(Debug, Clone, Default)]
pub struct SessionGate;

impl Gate for SessionGate {
    fn name(&self) -> &str {
        "session"
    }

    fn allows(&self, _task_name: &str, _now: DateTime<Local>, state: &GateState) -> bool {
        !state.session_active
    }
}

/// Block if another instance of the same task is mid-flight.
#[derive(Debug, Clone, Default)]
pub struct LockGate;

impl Gate for LockGate {
    fn name(&self) -> &str {
        "lock"
    }

    fn allows(&self, task_name: &str, _now: DateTime<Local>, state: &GateState) -> bool {
        !state.locks.get(task_name).copied().unwrap_or(false)
    }
}

/// Run only within a wall-clock window. Wraps midnight when
/// `start_hour > end_hour`.
#[derive(Debug, Clone)]
pub struct ActiveHoursGate {
    pub start_hour: u32,
    pub end_hour: u32,
}

impl ActiveHoursGate {
    pub fn new(start_hour: u32, end_hour: u32) -> Self {
        Self {
            start_hour,
            end_hour,
        }
    }
    pub fn default_window() -> Self {
        Self::new(9, 21)
    }
}

impl Gate for ActiveHoursGate {
    fn name(&self) -> &str {
        "active_hours"
    }

    fn allows(&self, _task_name: &str, now: DateTime<Local>, _state: &GateState) -> bool {
        let h = now.hour();
        if self.start_hour == self.end_hour {
            return true;
        }
        if self.start_hour < self.end_hour {
            h >= self.start_hour && h < self.end_hour
        } else {
            h >= self.start_hour || h < self.end_hour
        }
    }
}

/// Run only on specific weekdays.
#[derive(Debug, Clone)]
pub struct WeekdayGate {
    pub days: Vec<Weekday>,
}

impl WeekdayGate {
    pub fn new(days: Vec<Weekday>) -> Self {
        Self { days }
    }
}

impl Gate for WeekdayGate {
    fn name(&self) -> &str {
        "weekday"
    }

    fn allows(&self, _task_name: &str, now: DateTime<Local>, _state: &GateState) -> bool {
        self.days.contains(&now.weekday())
    }
}

// ─────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 4, 14, hour, 0, 0).unwrap()
    }

    #[test]
    fn time_gate_allows_first_run() {
        let gate = TimeGate::new(Duration::from_secs(3600));
        let state = GateState::new();
        assert!(gate.allows("x", at(12), &state));
    }

    #[test]
    fn time_gate_blocks_within_interval() {
        let gate = TimeGate::new(Duration::from_secs(3600));
        let mut state = GateState::new();
        state.record_run("x", at(12));
        let later = Local.with_ymd_and_hms(2026, 4, 14, 12, 30, 0).unwrap();
        assert!(!gate.allows("x", later, &state));
        assert!(gate.allows("x", at(14), &state));
    }

    #[test]
    fn session_gate_blocks_while_active() {
        let gate = SessionGate;
        let mut state = GateState::new();
        assert!(gate.allows("x", at(12), &state));
        state.session_active = true;
        assert!(!gate.allows("x", at(12), &state));
    }

    #[test]
    fn lock_gate_blocks_busy_task() {
        let gate = LockGate;
        let mut state = GateState::new();
        assert!(gate.allows("x", at(12), &state));
        assert!(state.try_acquire("x"));
        assert!(!gate.allows("x", at(12), &state));
        state.release("x");
        assert!(gate.allows("x", at(12), &state));
    }

    #[test]
    fn lock_gate_try_acquire_is_exclusive() {
        let mut state = GateState::new();
        assert!(state.try_acquire("job"));
        assert!(!state.try_acquire("job"));
        state.release("job");
        assert!(state.try_acquire("job"));
    }

    #[test]
    fn active_hours_normal_window() {
        let gate = ActiveHoursGate::new(9, 21);
        let state = GateState::new();
        assert!(!gate.allows("x", at(8), &state));
        assert!(gate.allows("x", at(9), &state));
        assert!(gate.allows("x", at(20), &state));
        assert!(!gate.allows("x", at(21), &state));
    }

    #[test]
    fn active_hours_wraps_midnight() {
        let gate = ActiveHoursGate::new(22, 6);
        let state = GateState::new();
        assert!(gate.allows("x", at(23), &state));
        assert!(gate.allows("x", at(2), &state));
        assert!(!gate.allows("x", at(10), &state));
    }

    #[test]
    fn active_hours_degenerate_always_on() {
        let gate = ActiveHoursGate::new(0, 0);
        let state = GateState::new();
        for h in 0..24 {
            assert!(gate.allows("x", at(h), &state));
        }
    }

    #[test]
    fn weekday_gate_filters_days() {
        let gate = WeekdayGate::new(vec![Weekday::Mon, Weekday::Wed]);
        let state = GateState::new();
        let tue = Local.with_ymd_and_hms(2026, 4, 14, 12, 0, 0).unwrap();
        assert!(!gate.allows("x", tue, &state));
        let wed = Local.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        assert!(gate.allows("x", wed, &state));
    }
}
