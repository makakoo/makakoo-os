//! Cron schedule parsing — the single source of truth for both
//! spec validation and the supervisor's trigger scheduler.
//!
//! Validation and execution MUST agree. If `agent create` accepts a
//! schedule the supervisor cannot parse, the agent is created, looks
//! healthy, and silently never fires — the worst failure mode for a
//! trigger, because nothing is there to observe its absence. Both
//! paths therefore call [`CronSchedule::parse`].
//!
//! Spec schedules are standard 5-field cron (`min hour dom mon dow`).
//! The `cron` crate expects a leading seconds field, so we prepend
//! `0` — a 5-field spec expression fires at second 0 of its minute.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{MakakooError, Result};

/// A parsed, timezone-bound cron schedule.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    /// Day-of-month branch (or the whole expression when only one of
    /// the day fields is restricted).
    schedule: cron::Schedule,
    /// Day-of-week branch, present only when *both* day fields are
    /// restricted. See [`CronSchedule::parse`] for why.
    dow_branch: Option<cron::Schedule>,
    tz: Tz,
    expr: String,
    tz_name: String,
}

impl CronSchedule {
    /// Parse a 5-field cron expression and an IANA timezone.
    ///
    /// An empty timezone means UTC.
    pub fn parse(expr: &str, timezone: &str) -> Result<Self> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(MakakooError::InvalidInput(format!(
                "cron schedule '{}' must have 5 space-separated fields (min hour dom mon dow), got {}",
                expr,
                fields.len()
            )));
        }

        let tz_name = if timezone.trim().is_empty() {
            "UTC"
        } else {
            timezone.trim()
        };
        let tz = Tz::from_str(tz_name).map_err(|_| {
            MakakooError::InvalidInput(format!(
                "cron timezone '{}' is not an IANA timezone (e.g. 'UTC', 'Europe/Berlin')",
                tz_name
            ))
        })?;

        // Prepend the seconds field the `cron` crate requires, and
        // translate day-of-week into the crate's numbering.
        let dow = translate_dow(fields[4]).ok_or_else(|| {
            MakakooError::InvalidInput(format!(
                "cron schedule '{}' has an invalid day-of-week field '{}' (expected 0-7, names, ranges, lists or steps)",
                expr, fields[4]
            ))
        })?;

        let build = |dom: &str, dow: &str| -> Result<cron::Schedule> {
            let text = format!(
                "0 {} {} {} {} {}",
                fields[0], fields[1], dom, fields[3], dow
            );
            cron::Schedule::from_str(&text).map_err(|e| {
                MakakooError::InvalidInput(format!("cron schedule '{}' is not valid: {e}", expr))
            })
        };

        // Vixie/POSIX cron ORs the day fields: when day-of-month AND
        // day-of-week are both restricted, a command runs if *either*
        // matches. The `cron` crate intersects them instead, so
        // `0 0 1 * 1` would mean "the 1st, but only when it is a
        // Monday" — roughly six firings a decade instead of sixty a
        // year, with nothing to indicate the schedule was misread.
        // Splitting into two schedules and taking the earlier candidate
        // restores the documented dialect.
        let dom_restricted = fields[2] != "*";
        let dow_restricted = fields[4] != "*";
        let (schedule, dow_branch) = if dom_restricted && dow_restricted {
            (build(fields[2], "*")?, Some(build("*", &dow)?))
        } else {
            (build(fields[2], &dow)?, None)
        };

        // A schedule that can never fire is a silent dead agent. Reject
        // it at create time rather than letting the supervisor idle
        // forever on a trigger that will not come.
        if schedule.upcoming(tz).next().is_none() && dow_branch.is_none() {
            return Err(MakakooError::InvalidInput(format!(
                "cron schedule '{}' never fires (no matching date exists)",
                expr
            )));
        }

        Ok(Self {
            schedule,
            dow_branch,
            tz,
            expr: expr.to_string(),
            tz_name: tz_name.to_string(),
        })
    }

    /// The next firing strictly after `after`, in UTC.
    ///
    /// Guaranteed to return a time strictly greater than `after`, or
    /// `None`. The crate iterates in *local* time, and converting a
    /// local candidate back to UTC across a DST fold can land on or
    /// before `after`; a caller that turned that into a zero-length
    /// sleep would spin hot for the length of the repeated hour. Each
    /// branch is therefore advanced until it is genuinely in the
    /// future.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let a = self.first_after(&self.schedule, after);
        let b = self
            .dow_branch
            .as_ref()
            .and_then(|s| self.first_after(s, after));
        match (a, b) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, None) => x,
            (None, y) => y,
        }
    }

    fn first_after(
        &self,
        schedule: &cron::Schedule,
        after: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        // A DST fold repeats at most one hour, so a small bound is
        // enough; it also stops a pathological schedule from iterating
        // forever.
        for candidate in schedule.after(&after.with_timezone(&self.tz)).take(64) {
            let utc = candidate.with_timezone(&Utc);
            if utc > after {
                return Some(utc);
            }
        }
        None
    }

    /// The original 5-field expression.
    pub fn expr(&self) -> &str {
        &self.expr
    }

    /// The resolved timezone name (never empty; `UTC` when unset).
    pub fn timezone(&self) -> &str {
        &self.tz_name
    }
}

/// Translate a standard-cron day-of-week field into the `cron` crate's
/// numbering.
///
/// Standard cron (POSIX/Vixie, and every example in `docs/agents/spec.md`)
/// numbers Sunday as 0, Monday as 1, with 7 also meaning Sunday. The `cron`
/// crate uses Quartz numbering: Sunday is 1, Monday is 2. Passing a spec
/// field through untranslated shifts every schedule back one day, silently
/// and permanently — `0 9 * * 1` would fire Sunday while its comment claims
/// Monday.
///
/// Ranges and steps are expanded to an explicit day list, which removes the
/// ambiguity around a `7` endpoint (`1-7` means the whole week in Vixie
/// cron, but would be an inverted range after a naive endpoint remap).
///
/// Name forms (`Mon`, `MON-FRI`) are passed through: the crate resolves
/// names to real weekdays, so they are already unambiguous.
fn translate_dow(field: &str) -> Option<String> {
    if field == "*" {
        return Some("*".to_string());
    }

    let mut out: Vec<String> = Vec::new();
    for token in field.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        // Names are unambiguous in both dialects — hand them over as-is.
        if token.chars().any(|c| c.is_ascii_alphabetic()) {
            out.push(token.to_string());
            continue;
        }

        let (range_part, step) = match token.split_once('/') {
            Some((r, s)) => (r, s.parse::<usize>().ok().filter(|s| *s > 0)?),
            None => (token, 1),
        };

        // Expand to raw standard-cron day numbers (0-7, 7 == Sunday).
        let raw: Vec<u32> = if range_part == "*" {
            (0..=6).collect()
        } else if let Some((lo, hi)) = range_part.split_once('-') {
            let lo: u32 = lo.parse().ok()?;
            let hi: u32 = hi.parse().ok()?;
            if lo > 7 || hi > 7 {
                return None;
            }
            if lo <= hi {
                (lo..=hi).collect()
            } else {
                // Wrapping range such as `5-1` (Fri..Mon).
                (lo..=7).chain(0..=hi).collect()
            }
        } else {
            let n: u32 = range_part.parse().ok()?;
            if n > 7 {
                return None;
            }
            vec![n]
        };

        // Normalise the Sunday aliases and drop duplicates BEFORE
        // stepping. A wrapping range such as `5-1` expands through both
        // 7 and 0; stepping the raw sequence would count Sunday twice
        // and shift every day after it (`5-1/2` would gain Monday).
        let mut traversal: Vec<u32> = Vec::new();
        for d in raw {
            let d = if d == 7 { 0 } else { d };
            if !traversal.contains(&d) {
                traversal.push(d);
            }
        }
        let mut days: Vec<u32> = traversal.into_iter().step_by(step).collect();
        days.sort_unstable();
        days.dedup();
        if days.is_empty() {
            return None;
        }
        // Standard 0-6 (Sun-Sat) -> crate 1-7 (Sun-Sat).
        out.push(
            days.iter()
                .map(|d| (d + 1).to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    if out.is_empty() {
        None
    } else {
        Some(out.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike, Weekday};

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    /// Pins the day-of-week convention. A silent off-by-one here moves
    /// every "Monday report" to Sunday, and nothing would fail loudly.
    #[test]
    fn dow_one_is_monday_matching_standard_cron() {
        let s = CronSchedule::parse("0 9 * * 1", "UTC").unwrap();
        let next = s.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
        assert_eq!(next.weekday(), Weekday::Mon, "dow=1 must be Monday");
        assert_eq!(next.hour(), 9);
        assert_eq!(next.minute(), 0);
    }

    /// Vixie cron ORs the day fields when both are restricted.
    /// Intersecting them turns "the 1st or any Monday" into "a Monday
    /// that is also the 1st" — about six firings a decade.
    #[test]
    fn a_restricted_dom_and_dow_are_ored_not_intersected() {
        let s = CronSchedule::parse("0 0 1 * 1", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        let mut hits = Vec::new();
        for _ in 0..6 {
            at = s.next_after(at).unwrap();
            hits.push(at);
        }
        // Every hit is either the 1st or a Monday...
        for h in &hits {
            assert!(
                h.day() == 1 || h.weekday() == Weekday::Mon,
                "unexpected firing {h}"
            );
        }
        // ...and the very next one lands within the coming week rather
        // than months away.
        assert!(
            hits[0] < utc(2026, 9, 5, 0, 0),
            "first firing {} is too far out — the fields were intersected",
            hits[0]
        );
        // Both branches must actually appear.
        assert!(hits.iter().any(|h| h.day() == 1), "no day-of-month firing");
        assert!(
            hits.iter()
                .any(|h| h.weekday() == Weekday::Mon && h.day() != 1),
            "no day-of-week firing"
        );
    }

    /// A restricted DOM with an unrestricted DOW must stay exact.
    #[test]
    fn a_day_of_month_only_schedule_is_not_widened_by_the_union() {
        let s = CronSchedule::parse("0 0 1 * *", "UTC").unwrap();
        for _ in 0..4 {
            let n = s.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
            assert_eq!(n.day(), 1);
        }
    }

    /// `next_after` must never return a time <= `after`; a caller turns
    /// that into a zero-length sleep and spins hot.
    #[test]
    fn next_after_is_strictly_monotonic_across_a_dst_fold() {
        // Europe/Berlin falls back on 2026-10-25: 02:00-03:00 repeats.
        let s = CronSchedule::parse("30 2 * * *", "Europe/Berlin").unwrap();
        let mut at = utc(2026, 10, 24, 12, 0);
        for _ in 0..8 {
            let next = s.next_after(at).unwrap();
            assert!(next > at, "next_after returned {next} which is <= {at}");
            at = next;
        }
    }

    #[test]
    fn next_after_is_strictly_monotonic_for_a_frequent_schedule() {
        let s = CronSchedule::parse("* * * * *", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 9, 0);
        for _ in 0..120 {
            let next = s.next_after(at).unwrap();
            assert!(next > at);
            at = next;
        }
    }

    /// A wrapping range expands through both 7 and 0; stepping the raw
    /// sequence would count Sunday twice and shift the days after it.
    #[test]
    fn a_stepped_wrapping_dow_range_does_not_gain_a_day() {
        // 5-1 is Fri,Sat,Sun,Mon; step 2 selects Fri and Sun.
        let s = CronSchedule::parse("0 9 * * 5-1/2", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            at = s.next_after(at).unwrap();
            seen.insert(at.weekday());
        }
        assert!(seen.contains(&Weekday::Fri), "{seen:?}");
        assert!(seen.contains(&Weekday::Sun), "{seen:?}");
        assert!(
            !seen.contains(&Weekday::Mon),
            "Monday leaked in from double-counting Sunday: {seen:?}"
        );
        assert!(!seen.contains(&Weekday::Sat), "{seen:?}");
    }
    #[test]
    fn dow_zero_is_sunday() {
        let s = CronSchedule::parse("0 9 * * 0", "UTC").unwrap();
        let next = s.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
        assert_eq!(next.weekday(), Weekday::Sun, "dow=0 must be Sunday");
    }

    /// Every weekday number in the spec dialect must land on the day the
    /// spec author wrote down. This is the regression for the Quartz
    /// numbering mismatch in the `cron` crate.
    #[test]
    fn every_standard_dow_number_maps_to_the_right_weekday() {
        let expected = [
            (0, Weekday::Sun),
            (1, Weekday::Mon),
            (2, Weekday::Tue),
            (3, Weekday::Wed),
            (4, Weekday::Thu),
            (5, Weekday::Fri),
            (6, Weekday::Sat),
            (7, Weekday::Sun), // 7 is Sunday too, per Vixie cron.
        ];
        for (n, want) in expected {
            let s = CronSchedule::parse(&format!("0 9 * * {n}"), "UTC").unwrap();
            let got = s.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
            assert_eq!(got.weekday(), want, "dow={n} should be {want:?}");
        }
    }

    /// The exact expression shipped in examples/agents/scheduled-reporter.yaml,
    /// whose comment promises "09:00 every Monday".
    #[test]
    fn the_shipped_weekly_reporter_example_fires_on_monday() {
        let s = CronSchedule::parse("0 9 * * 1", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        for _ in 0..5 {
            at = s.next_after(at).unwrap();
            assert_eq!(at.weekday(), Weekday::Mon);
            assert_eq!(at.hour(), 9);
        }
    }

    #[test]
    fn a_weekday_range_covers_monday_through_friday_only() {
        let s = CronSchedule::parse("0 9 * * 1-5", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        let mut seen = Vec::new();
        for _ in 0..7 {
            at = s.next_after(at).unwrap();
            seen.push(at.weekday());
        }
        assert!(!seen.contains(&Weekday::Sat), "{seen:?}");
        assert!(!seen.contains(&Weekday::Sun), "{seen:?}");
        assert!(
            seen.contains(&Weekday::Mon) && seen.contains(&Weekday::Fri),
            "{seen:?}"
        );
    }

    #[test]
    fn a_weekend_list_covers_saturday_and_sunday_only() {
        let s = CronSchedule::parse("0 9 * * 0,6", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        for _ in 0..6 {
            at = s.next_after(at).unwrap();
            assert!(
                matches!(at.weekday(), Weekday::Sat | Weekday::Sun),
                "got {:?}",
                at.weekday()
            );
        }
    }

    /// Vixie cron reads `1-7` as the whole week; a naive endpoint remap
    /// would turn it into an inverted range.
    #[test]
    fn a_full_week_range_ending_at_seven_covers_every_day() {
        let s = CronSchedule::parse("0 9 * * 1-7", "UTC").unwrap();
        let mut at = utc(2026, 8, 28, 0, 0);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            at = s.next_after(at).unwrap();
            seen.insert(at.weekday());
        }
        assert_eq!(seen.len(), 7, "expected all 7 weekdays, got {seen:?}");
    }

    #[test]
    fn weekday_names_still_resolve_correctly() {
        let s = CronSchedule::parse("0 9 * * Mon", "UTC").unwrap();
        let got = s.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
        assert_eq!(got.weekday(), Weekday::Mon);
    }

    #[test]
    fn an_out_of_range_dow_is_refused() {
        let err = CronSchedule::parse("0 9 * * 8", "UTC").unwrap_err();
        assert!(err.to_string().contains("day-of-week"), "{err}");
    }

    #[test]
    fn a_daily_schedule_advances_one_day_at_a_time() {
        let s = CronSchedule::parse("0 8 * * *", "UTC").unwrap();
        let first = s.next_after(utc(2026, 8, 28, 9, 0)).unwrap();
        assert_eq!((first.day(), first.hour()), (29, 8));
        let second = s.next_after(first).unwrap();
        assert_eq!((second.day(), second.hour()), (30, 8));
    }

    /// The whole point of the timezone field: 08:00 Berlin is not 08:00 UTC.
    #[test]
    fn a_local_timezone_shifts_the_utc_firing_time() {
        let berlin = CronSchedule::parse("0 8 * * *", "Europe/Berlin").unwrap();
        let next = berlin.next_after(utc(2026, 8, 28, 0, 0)).unwrap();
        // Berlin is UTC+2 in August (CEST).
        assert_eq!(next.hour(), 6, "08:00 CEST must be 06:00 UTC");
    }

    #[test]
    fn an_empty_timezone_means_utc() {
        let s = CronSchedule::parse("0 8 * * *", "").unwrap();
        assert_eq!(s.timezone(), "UTC");
        assert_eq!(s.next_after(utc(2026, 8, 28, 0, 0)).unwrap().hour(), 8);
    }

    #[test]
    fn a_bogus_timezone_is_refused_rather_than_silently_becoming_utc() {
        // Silently defaulting would fire the agent at the wrong hour
        // forever with no error anywhere.
        let err = CronSchedule::parse("0 8 * * *", "Europe/Berlinn").unwrap_err();
        assert!(err.to_string().contains("not an IANA timezone"), "{err}");
    }

    #[test]
    fn a_six_field_expression_is_refused() {
        let err = CronSchedule::parse("0 0 8 * * *", "UTC").unwrap_err();
        assert!(err.to_string().contains("5 space-separated"), "{err}");
    }

    #[test]
    fn a_schedule_that_can_never_fire_is_refused_at_parse_time() {
        // 30 February.
        let err = CronSchedule::parse("0 0 30 2 *", "UTC").unwrap_err();
        assert!(err.to_string().contains("never fires"), "{err}");
    }

    #[test]
    fn step_and_list_syntax_parses() {
        let s = CronSchedule::parse("*/15 * * * *", "UTC").unwrap();
        let a = s.next_after(utc(2026, 8, 28, 9, 1)).unwrap();
        assert_eq!(a.minute(), 15);
        let s2 = CronSchedule::parse("0 9,17 * * 1-5", "UTC").unwrap();
        assert!(s2.next_after(utc(2026, 8, 28, 0, 0)).is_some());
    }

    /// Spring-forward: 02:30 local does not exist on that date in Berlin.
    /// The schedule must still make progress rather than stall or panic.
    #[test]
    fn a_dst_gap_does_not_stall_the_schedule() {
        let s = CronSchedule::parse("30 2 * * *", "Europe/Berlin").unwrap();
        // 2027-03-28 is the European spring-forward date.
        let next = s.next_after(utc(2027, 3, 27, 12, 0)).unwrap();
        let after = s.next_after(next).unwrap();
        assert!(after > next, "schedule must advance across a DST gap");
    }
}
