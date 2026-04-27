//! `makakoo buddy status` — render the active mascot's frame.

use crate::cli::BuddyCmd;
use crate::context::CliContext;
use crate::output;

pub fn run(ctx: &CliContext, cmd: BuddyCmd) -> anyhow::Result<i32> {
    match cmd {
        BuddyCmd::Status => {
            let buddy = ctx.buddy()?;
            let frame = buddy.display_frame();
            output::print_buddy_frame(&frame);
            Ok(0)
        }
    }
}
