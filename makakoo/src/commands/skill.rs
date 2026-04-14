//! `makakoo skill <name> [args...]` — Python skill subprocess bridge.

use crate::output;
use crate::skill_runner::SkillRunner;

pub fn run(name: &str, args: &[String]) -> anyhow::Result<i32> {
    let runner = SkillRunner::new()?;
    match runner.run(name, args) {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(e) => {
            output::print_error(format!("skill '{name}': {e}"));
            Ok(1)
        }
    }
}
