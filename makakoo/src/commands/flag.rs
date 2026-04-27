//! `makakoo flag` — manual GYM Layer 1 producer.

use makakoo_core::gym::{ErrorCapture, ErrorEntry, ErrorSource};

use crate::context::CliContext;

pub fn run(ctx: &CliContext, reason: &str, skill: Option<String>) -> anyhow::Result<i32> {
    let cap = ErrorCapture::new(ctx.home());
    let mut entry = ErrorEntry::new(ErrorSource::ManualFlag)
        .cmd("makakoo flag")
        .stderr(reason)
        .error_class("skill");
    if let Some(s) = skill {
        entry = entry.skill_in_scope(s);
    }
    if cap.record(entry) {
        println!("flagged: {reason}");
        Ok(0)
    } else {
        eprintln!("flag capture failed (silently — see GYM funnel logs)");
        Ok(1)
    }
}
