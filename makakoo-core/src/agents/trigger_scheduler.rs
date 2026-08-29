//! Cron trigger scheduler — wakes an agent with no human in the loop.
//!
//! This is the counterpart to [`transport_bridge`](super::transport_bridge):
//! the bridge carries messages *in* from a person, the scheduler starts a
//! turn because a clock said so. Both are hosted by the supervisor and
//! both terminate on the same shutdown signal.
//!
//! Design notes that are easy to get wrong:
//!
//! * **Ticks are skipped, never queued.** Each loop computes the next
//!   firing from `Utc::now()` *after* the previous run returns. A run that
//!   overruns its own period therefore drops the ticks it covered instead
//!   of building a backlog that can never drain.
//! * **A sleeping laptop does not cause a storm.** Waking up hours late
//!   recomputes the next *future* tick rather than replaying every missed
//!   one. A tick that is late by more than [`MISSED_GRACE`] is reported
//!   and dropped: a 08:00 morning brief delivered at 16:00 is worse than
//!   no brief.
//! * **Delivery is allowlist-bound.** Scheduled output goes only to ids
//!   already authorised to talk to the agent.
//! * **A headless agent is legitimate.** With no channels the answer is
//!   logged, not delivered — that is the `scheduled-reporter` shape,
//!   where the agent's work is its filesystem side effects.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinHandle;

use crate::agents::runtime_client::{self, RunOutcome};
use crate::agents::schedule::CronSchedule;
use crate::agents::slot::AgentSlot;
use crate::agents::supervisor_runtime::ShutdownSignal;
use crate::agents::transport_bridge::DeliveryTarget;
use crate::Result;

/// How long a run may take before the scheduler gives up on it. Matches
/// the bridge's per-message budget.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// A tick later than this is dropped rather than run. Covers a brief
/// scheduling delay or a short suspend, not an overnight sleep.
const MISSED_GRACE: Duration = Duration::from_secs(300);

/// How long to wait for the runtime to publish its endpoint before
/// treating a tick as unrunnable.
const ENDPOINT_WAIT: Duration = Duration::from_secs(30);
const ENDPOINT_POLL: Duration = Duration::from_millis(500);

/// A trigger the supervisor declined to schedule, with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedTrigger {
    pub trigger_id: String,
    pub reason: String,
}

/// One parsed, runnable trigger.
#[derive(Debug, Clone)]
struct ScheduledTrigger {
    id: String,
    schedule: CronSchedule,
    prompt: String,
    deliver_to: Vec<String>,
}

/// What the supervisor should run for a slot's triggers.
pub struct TriggerPlan {
    pub scheduler: Option<TriggerScheduler>,
    pub skipped: Vec<SkippedTrigger>,
}

/// Owns every trigger for one slot.
pub struct TriggerScheduler {
    slot_id: String,
    project_dir: PathBuf,
    triggers: Vec<ScheduledTrigger>,
    targets: Vec<DeliveryTarget>,
    http: reqwest::Client,
}

impl TriggerScheduler {
    /// Decide what to schedule for `slot`.
    ///
    /// A malformed trigger is skipped with a reason rather than failing
    /// the slot: one bad schedule must not stop an agent whose other
    /// triggers and channels are fine.
    pub fn plan(slot: &AgentSlot, targets: Vec<DeliveryTarget>) -> Result<TriggerPlan> {
        let Some(runtime) = slot.runtime.as_ref() else {
            // Legacy gateway slots have no runtime to call.
            return Ok(TriggerPlan {
                scheduler: None,
                skipped: slot
                    .triggers
                    .iter()
                    .filter(|t| t.enabled)
                    .map(|t| SkippedTrigger {
                        trigger_id: t.id.clone(),
                        reason: "slot has no compiled runtime to wake".into(),
                    })
                    .collect(),
            });
        };

        let mut triggers = Vec::new();
        let mut skipped = Vec::new();

        for t in &slot.triggers {
            if !t.enabled {
                skipped.push(SkippedTrigger {
                    trigger_id: t.id.clone(),
                    reason: "disabled".into(),
                });
                continue;
            }
            if t.kind != "cron" {
                skipped.push(SkippedTrigger {
                    trigger_id: t.id.clone(),
                    reason: format!(
                        "kind '{}' has no supervisor-hosted scheduler yet — only cron is scheduled",
                        t.kind
                    ),
                });
                continue;
            }
            let schedule = match CronSchedule::parse(&t.schedule, &t.timezone) {
                Ok(s) => s,
                Err(e) => {
                    skipped.push(SkippedTrigger {
                        trigger_id: t.id.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            // An unknown delivery target is a typo. Report it loudly,
            // but do NOT suppress the trigger: refusing to run the turn
            // would also cancel the agent's filesystem side effects,
            // which is a bigger failure than a misrouted reply.
            let known: Vec<String> = targets
                .iter()
                .map(|d| d.transport_id().to_string())
                .collect();
            let unknown: Vec<&String> =
                t.deliver_to.iter().filter(|d| !known.contains(d)).collect();
            if !unknown.is_empty() {
                skipped.push(SkippedTrigger {
                    trigger_id: t.id.clone(),
                    reason: format!(
                        "deliver_to names unknown transport(s) {:?} (available: {:?}) — \
                         the trigger still runs and delivers to the rest",
                        unknown, known
                    ),
                });
            }

            triggers.push(ScheduledTrigger {
                id: t.id.clone(),
                schedule,
                prompt: if t.prompt.trim().is_empty() {
                    crate::agents::spec::DEFAULT_CRON_PROMPT.to_string()
                } else {
                    t.prompt.clone()
                },
                deliver_to: t.deliver_to.clone(),
            });
        }

        if triggers.is_empty() {
            return Ok(TriggerPlan {
                scheduler: None,
                skipped,
            });
        }

        Ok(TriggerPlan {
            scheduler: Some(Self {
                slot_id: slot.slot_id.clone(),
                project_dir: runtime.project_dir.clone(),
                triggers,
                targets,
                http: reqwest::Client::new(),
            }),
            skipped,
        })
    }

    /// Trigger ids this scheduler will run.
    pub fn trigger_ids(&self) -> Vec<String> {
        self.triggers.iter().map(|t| t.id.clone()).collect()
    }

    /// Spawn one task per trigger. Each returns on shutdown.
    pub fn spawn(self, shutdown: ShutdownSignal) -> Vec<JoinHandle<()>> {
        let targets = Arc::new(self.targets);
        self.triggers
            .into_iter()
            .map(|trigger| {
                let slot_id = self.slot_id.clone();
                let project_dir = self.project_dir.clone();
                let http = self.http.clone();
                let targets = targets.clone();
                let mut shutdown = shutdown.clone();
                tokio::spawn(async move {
                    run_trigger(slot_id, project_dir, http, targets, trigger, &mut shutdown).await;
                })
            })
            .collect()
    }
}

/// What to do with a tick that has come due.
#[derive(Debug, Clone, PartialEq)]
pub enum TickAction {
    /// Fire the trigger.
    Run,
    /// Too late to be useful — drop it and wait for the next one.
    Drop { late_seconds: i64 },
    /// The wall clock is behind the due time: the timer elapsed but the
    /// moment has not arrived. Wait again rather than firing early.
    Early { remaining: chrono::Duration },
}

/// Decide whether a tick due at `due` should still run at `now`.
///
/// Split out from the sleep loop so the suspend-and-wake behaviour is
/// testable without waiting on a real clock.
pub fn tick_action(due: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> TickAction {
    let delta = now.signed_duration_since(due);
    // The sleep is measured on a monotonic timer but the schedule lives
    // in wall time, and the two disagree across suspend and NTP steps.
    // If the clock stepped backwards the timer still fires; treating
    // that as "on time" would run the tick early and then run the very
    // same tick again when its real moment arrived.
    if delta < chrono::Duration::zero() {
        return TickAction::Early { remaining: -delta };
    }
    // Compared as an exact duration: truncating to whole seconds would
    // let a tick 300.9s late through a 300s grace.
    let grace = chrono::Duration::from_std(MISSED_GRACE).expect("grace fits");
    if delta > grace {
        TickAction::Drop {
            late_seconds: delta.num_seconds(),
        }
    } else {
        TickAction::Run
    }
}

async fn run_trigger(
    slot_id: String,
    project_dir: PathBuf,
    http: reqwest::Client,
    targets: Arc<Vec<DeliveryTarget>>,
    trigger: ScheduledTrigger,
    shutdown: &mut ShutdownSignal,
) {
    tracing::info!(
        target: "makakoo_core::agents::trigger_scheduler",
        slot = slot_id,
        trigger = trigger.id,
        schedule = trigger.schedule.expr(),
        timezone = trigger.schedule.timezone(),
        "cron trigger scheduled"
    );

    loop {
        let now = Utc::now();
        let Some(next) = trigger.schedule.next_after(now) else {
            tracing::warn!(
                target: "makakoo_core::agents::trigger_scheduler",
                slot = slot_id,
                trigger = trigger.id,
                "cron schedule has no further firings — trigger retiring"
            );
            return;
        };

        let wait = (next - now).to_std().unwrap_or(Duration::ZERO);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.wait() => return,
        }

        // The wall clock may have jumped while we slept (suspend, NTP
        // step). Replaying a long-past tick is worse than dropping it.
        match tick_action(next, Utc::now()) {
            TickAction::Run => {}
            TickAction::Drop { late_seconds } => {
                tracing::warn!(
                    target: "makakoo_core::agents::trigger_scheduler",
                    slot = slot_id,
                    trigger = trigger.id,
                    due = %next,
                    late_seconds,
                    "missed cron tick dropped — the machine was likely asleep"
                );
                continue;
            }
            TickAction::Early { remaining } => {
                tracing::warn!(
                    target: "makakoo_core::agents::trigger_scheduler",
                    slot = slot_id,
                    trigger = trigger.id,
                    due = %next,
                    remaining_seconds = remaining.num_seconds(),
                    "cron tick came due early — the wall clock stepped backwards"
                );
                continue;
            }
        }

        fire(&slot_id, &project_dir, &http, &targets, &trigger, shutdown).await;

        if shutdown.is_fired() {
            return;
        }
    }
}

async fn fire(
    slot_id: &str,
    project_dir: &Path,
    http: &reqwest::Client,
    targets: &[DeliveryTarget],
    trigger: &ScheduledTrigger,
    shutdown: &mut ShutdownSignal,
) {
    // The runtime's port is ephemeral, so the endpoint is re-read on
    // every tick rather than cached across the agent's whole lifetime.
    let endpoint = match wait_for_endpoint(project_dir, slot_id, shutdown).await {
        Some(e) => e,
        None => return,
    };

    // A stable session id gives the schedule its own conversation
    // history, separate from every chat, and preserved across restarts.
    let session_id = format!("cron:{}", trigger.id);

    let outcome = tokio::select! {
        r = runtime_client::run_prompt(http, &endpoint, &trigger.prompt, &session_id, RUN_TIMEOUT) => r,
        _ = shutdown.wait() => return,
    };

    let answer = match outcome {
        Ok(RunOutcome::Answer(text)) => text,
        Ok(RunOutcome::Refused { status, message }) => {
            tracing::error!(
                target: "makakoo_core::agents::trigger_scheduler",
                slot = slot_id,
                trigger = trigger.id,
                status,
                detail = message,
                "scheduled run refused by the runtime"
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                target: "makakoo_core::agents::trigger_scheduler",
                slot = slot_id,
                trigger = trigger.id,
                error = %e,
                "scheduled run failed"
            );
            return;
        }
    };

    deliver(slot_id, targets, trigger, &answer).await;
}

async fn deliver(
    slot_id: &str,
    targets: &[DeliveryTarget],
    trigger: &ScheduledTrigger,
    answer: &str,
) {
    let selected: Vec<&DeliveryTarget> = if trigger.deliver_to.is_empty() {
        targets.iter().collect()
    } else {
        targets
            .iter()
            .filter(|t| trigger.deliver_to.iter().any(|d| d == t.transport_id()))
            .collect()
    };

    if selected.is_empty() {
        // Headless by design: the agent's output is its side effects.
        tracing::info!(
            target: "makakoo_core::agents::trigger_scheduler",
            slot = slot_id,
            trigger = trigger.id,
            chars = answer.chars().count(),
            "scheduled run completed with no delivery channel — the full answer is in \
             the runtime session transcript, not this log"
        );
        return;
    }

    for target in selected {
        if target.chats().is_empty() {
            tracing::warn!(
                target: "makakoo_core::agents::trigger_scheduler",
                slot = slot_id,
                trigger = trigger.id,
                transport_id = target.transport_id(),
                "transport has an empty allowlist — nowhere authorised to deliver"
            );
            continue;
        }
        for chat in target.chats() {
            if let Err(e) = target.send_text(chat, answer).await {
                tracing::error!(
                    target: "makakoo_core::agents::trigger_scheduler",
                    slot = slot_id,
                    trigger = trigger.id,
                    transport_id = target.transport_id(),
                    error = %e,
                    "scheduled delivery failed"
                );
            }
        }
    }
}

/// Wait briefly for the runtime endpoint. A tick that lands during a
/// runtime restart should not be lost to a race.
async fn wait_for_endpoint(
    project_dir: &Path,
    slot_id: &str,
    shutdown: &mut ShutdownSignal,
) -> Option<runtime_client::RuntimeEndpoint> {
    let deadline = tokio::time::Instant::now() + ENDPOINT_WAIT;
    // Assigned by the match below before any read; declaring it without
    // an initialiser keeps the "never read" lint honest.
    let mut last: String;
    loop {
        match runtime_client::read_endpoint(project_dir, slot_id) {
            Ok(e) => return Some(e),
            Err(e) => last = e.to_string(),
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::error!(
                target: "makakoo_core::agents::trigger_scheduler",
                slot = slot_id,
                error = last,
                "runtime endpoint unavailable — skipping this tick"
            );
            return None;
        }
        tokio::select! {
            _ = tokio::time::sleep(ENDPOINT_POLL) => {}
            _ = shutdown.wait() => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::slot::{AgentRuntime, AgentRuntimeEngine, TriggerEntry};
    use crate::transport::config::{TelegramConfig, TransportConfig, TransportEntry};
    use crate::transport::secrets::tests::MemSecrets;
    use chrono::{Duration as ChronoDuration, TimeZone};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn trigger(id: &str, schedule: &str) -> TriggerEntry {
        TriggerEntry {
            id: id.into(),
            kind: "cron".into(),
            enabled: true,
            schedule: schedule.into(),
            timezone: "UTC".into(),
            prompt: String::new(),
            deliver_to: Vec::new(),
        }
    }

    fn slot(triggers: Vec<TriggerEntry>) -> AgentSlot {
        AgentSlot {
            slot_id: "reporter".into(),
            name: "reporter".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: "supervised_pair".into(),
            transports: vec![],
            llm: None,
            runtime: Some(AgentRuntime {
                engine: AgentRuntimeEngine::DeepseekHarness,
                project_dir: PathBuf::from("/tmp/reporter"),
            }),
            triggers,
        }
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn a_valid_cron_trigger_is_scheduled() {
        let plan = TriggerScheduler::plan(&slot(vec![trigger("cron-1", "0 9 * * 1")]), vec![])
            .expect("plans");
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        assert_eq!(
            plan.scheduler.expect("scheduler").trigger_ids(),
            vec!["cron-1".to_string()]
        );
    }

    #[test]
    fn an_empty_prompt_becomes_the_default_wake_message() {
        let plan =
            TriggerScheduler::plan(&slot(vec![trigger("cron-1", "0 9 * * *")]), vec![]).unwrap();
        let s = plan.scheduler.unwrap();
        assert_eq!(
            s.triggers[0].prompt,
            crate::agents::spec::DEFAULT_CRON_PROMPT
        );
        assert!(!s.triggers[0].prompt.trim().is_empty());
    }

    #[test]
    fn a_disabled_trigger_is_skipped_with_a_reason() {
        let mut t = trigger("cron-1", "0 9 * * *");
        t.enabled = false;
        let plan = TriggerScheduler::plan(&slot(vec![t]), vec![]).unwrap();
        assert!(plan.scheduler.is_none());
        assert_eq!(plan.skipped[0].reason, "disabled");
    }

    /// One malformed schedule must not stop the agent's other triggers.
    #[test]
    fn a_broken_schedule_is_skipped_without_taking_down_the_good_one() {
        let bad = trigger("cron-bad", "not a cron expression");
        let good = trigger("cron-good", "0 9 * * *");
        let plan = TriggerScheduler::plan(&slot(vec![bad, good]), vec![]).unwrap();
        assert_eq!(
            plan.scheduler.expect("scheduler").trigger_ids(),
            vec!["cron-good".to_string()]
        );
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].trigger_id, "cron-bad");
    }

    /// A schedule the validator would reject must also be refused here —
    /// slot TOML can be hand-edited after `agent create`.
    #[test]
    fn a_hand_edited_bogus_timezone_is_refused_at_plan_time() {
        let mut t = trigger("cron-1", "0 9 * * *");
        t.timezone = "Mars/Olympus".into();
        let plan = TriggerScheduler::plan(&slot(vec![t]), vec![]).unwrap();
        assert!(plan.scheduler.is_none());
        assert!(
            plan.skipped[0].reason.contains("IANA"),
            "{}",
            plan.skipped[0].reason
        );
    }

    #[test]
    fn a_non_cron_kind_is_skipped_and_says_only_cron_is_hosted() {
        let mut t = trigger("hook-1", "0 9 * * *");
        t.kind = "webhook".into();
        let plan = TriggerScheduler::plan(&slot(vec![t]), vec![]).unwrap();
        assert!(plan.scheduler.is_none());
        assert!(plan.skipped[0].reason.contains("only cron"));
    }

    #[test]
    fn a_slot_without_a_runtime_cannot_be_woken() {
        let mut s = slot(vec![trigger("cron-1", "0 9 * * *")]);
        s.runtime = None;
        let plan = TriggerScheduler::plan(&s, vec![]).unwrap();
        assert!(plan.scheduler.is_none());
        assert!(plan.skipped[0].reason.contains("no compiled runtime"));
    }

    #[test]
    fn a_punctual_tick_runs() {
        let due = utc(2026, 8, 28, 9, 0);
        assert_eq!(tick_action(due, due), TickAction::Run);
        assert_eq!(
            tick_action(due, due + ChronoDuration::seconds(30)),
            TickAction::Run
        );
    }

    /// The laptop case: sleeping through 08:00 must not deliver a
    /// "morning brief" at 16:00, and must not replay every missed day.
    #[test]
    fn a_tick_missed_by_a_suspend_is_dropped_not_replayed() {
        let due = utc(2026, 8, 28, 8, 0);
        let woke = utc(2026, 8, 28, 16, 0);
        match tick_action(due, woke) {
            TickAction::Drop { late_seconds } => assert_eq!(late_seconds, 8 * 3600),
            other => panic!("expected Drop, got {other:?}"),
        }
    }

    #[test]
    fn a_tick_within_the_grace_window_still_runs() {
        let due = utc(2026, 8, 28, 8, 0);
        let late = due + ChronoDuration::seconds(MISSED_GRACE.as_secs() as i64 - 1);
        assert_eq!(tick_action(due, late), TickAction::Run);
    }

    /// A stale `deliver_to` entry must not cancel the agent's turn: the
    /// filesystem side effects matter more than one misrouted reply.
    #[tokio::test]
    async fn an_unknown_delivery_target_is_reported_but_still_runs_the_trigger() {
        let telegram = MockServer::start().await;
        let targets = delivery_targets(&telegram).await;
        let mut t = trigger("cron-1", "0 9 * * *");
        t.deliver_to = vec!["telegram-main".into(), "telegram-typo".into()];
        let plan = TriggerScheduler::plan(&slot(vec![t]), targets).unwrap();
        assert_eq!(
            plan.scheduler
                .expect("trigger still scheduled")
                .trigger_ids(),
            vec!["cron-1".to_string()]
        );
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].reason.contains("still runs"));
    }

    /// A clock that steps backwards must not fire the tick early and
    /// then fire the same tick again at its real moment.
    #[test]
    fn a_backwards_clock_step_does_not_fire_early() {
        let due = utc(2026, 8, 28, 9, 0);
        let before = utc(2026, 8, 28, 8, 0);
        match tick_action(due, before) {
            TickAction::Early { remaining } => assert_eq!(remaining.num_seconds(), 3600),
            other => panic!("expected Early, got {other:?}"),
        }
    }

    /// Truncating to whole seconds would let a tick 300.9s late through
    /// a 300s grace.
    #[test]
    fn the_grace_boundary_is_not_truncated_to_whole_seconds() {
        let due = utc(2026, 8, 28, 9, 0);
        let just_over = due
            + ChronoDuration::seconds(MISSED_GRACE.as_secs() as i64)
            + ChronoDuration::milliseconds(900);
        assert!(
            matches!(tick_action(due, just_over), TickAction::Drop { .. }),
            "a tick past the grace must be dropped"
        );
        let exactly = due + ChronoDuration::seconds(MISSED_GRACE.as_secs() as i64);
        assert_eq!(tick_action(due, exactly), TickAction::Run);
    }

    // ── delivery ────────────────────────────────────────────────────

    async fn delivery_targets(telegram: &MockServer) -> Vec<DeliveryTarget> {
        let entry = TransportEntry {
            id: "telegram-main".into(),
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("TEST-BOT-TOKEN".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec!["4242".into()],
            config: TransportConfig::Telegram(TelegramConfig::default()),
        };
        let mut s = slot(vec![]);
        s.transports = vec![entry];
        let plan = crate::agents::transport_bridge::TransportBridge::plan_with_api_base(
            &s,
            &MemSecrets::with(&[]),
            Some(&telegram.uri()),
        )
        .expect("plans");
        plan.bridge.expect("bridge").delivery_targets()
    }

    /// Scheduled output must reach the allowlisted chat — the whole
    /// point of a morning brief.
    #[tokio::test]
    async fn a_scheduled_answer_is_delivered_to_the_allowlisted_chat() {
        let telegram = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botTEST-BOT-TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"message_id": 1}
            })))
            .expect(1)
            .mount(&telegram)
            .await;

        let targets = delivery_targets(&telegram).await;
        let t = ScheduledTrigger {
            id: "cron-1".into(),
            schedule: CronSchedule::parse("0 9 * * *", "UTC").unwrap(),
            prompt: "wake".into(),
            deliver_to: vec![],
        };
        deliver("reporter", &targets, &t, "the morning brief").await;
        telegram.verify().await;
    }

    /// A headless agent (no channels) is a supported shape, not an
    /// error: `scheduled-reporter` writes files instead of replying.
    #[tokio::test]
    async fn a_headless_agent_completes_without_a_delivery_channel() {
        let t = ScheduledTrigger {
            id: "cron-1".into(),
            schedule: CronSchedule::parse("0 9 * * *", "UTC").unwrap(),
            prompt: "wake".into(),
            deliver_to: vec![],
        };
        // No panic, no send, no target.
        deliver("reporter", &[], &t, "report written to disk").await;
    }

    /// Delivery must not fan out to transports the trigger did not name.
    #[tokio::test]
    async fn deliver_to_selects_only_the_named_transport() {
        let telegram = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botTEST-BOT-TOKEN/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": {"message_id": 1}
            })))
            .expect(0)
            .mount(&telegram)
            .await;

        let targets = delivery_targets(&telegram).await;
        let t = ScheduledTrigger {
            id: "cron-1".into(),
            schedule: CronSchedule::parse("0 9 * * *", "UTC").unwrap(),
            prompt: "wake".into(),
            // Names a transport that exists in the slot but is not this one.
            deliver_to: vec!["telegram-other".into()],
        };
        deliver("reporter", &targets, &t, "should not be sent").await;
        telegram.verify().await;
    }
}
