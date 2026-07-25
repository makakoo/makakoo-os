//! Cron trigger renderer — uses `node-cron` directly.
//!
//! V1: per codex review, `@flue/runtime` has NO `defineTrigger`
//! export. The canonical Flue pattern for scheduled work is
//! `ExtensionAPI.schedule()` / `scheduleEvery()`. For cron
//! expressions specifically, we use `node-cron` (already a
//! dep) which supports standard 5-field cron. The trigger
//! module is a side-effecting import: when the Flue runtime
//! loads the agent, it also loads this module, which starts
//! the scheduler.

pub fn render(schedule: &str, timezone: &str) -> anyhow::Result<String> {
    let tz = if timezone.is_empty() { "UTC" } else { timezone };
    Ok(format!(
        r##"/* AUTO-GENERATED. Do not edit by hand.
 *
 * NOTE: per codex review of Phase 4, @flue/runtime has no
 * defineTrigger. This module uses node-cron directly and is
 * loaded as a side effect from src/agents/assistant.ts.
 */
import cron from 'node-cron';
import {{ dispatch }} from '@flue/runtime';
import assistant from '../agents/assistant.ts';

const SCHEDULE = '{schedule}';
const TIMEZONE = '{tz}';

cron.schedule(
  SCHEDULE,
  async () => {{
    const id = `cron-${{Date.now()}}`;
    await dispatch(assistant, {{
      id,
      input: {{
        type: 'cron.tick',
        firedAt: new Date().toISOString(),
        schedule: SCHEDULE,
        timezone: TIMEZONE,
      }},
    }});
  }},
  {{ timezone: TIMEZONE }},
);
"##,
        schedule = schedule,
        tz = tz,
    ))
}
