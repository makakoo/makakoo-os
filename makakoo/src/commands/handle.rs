use makakoo_core::agent_session::{self, AgentSessionStore, ReadMode};

use crate::cli::HandleCmd;
use crate::context::CliContext;

pub fn run(ctx: &CliContext, cmd: HandleCmd) -> anyhow::Result<i32> {
    let store = AgentSessionStore::open(&agent_session::db_path(ctx.home()), ctx.event_bus().ok())?;
    match cmd {
        HandleCmd::Read {
            handle,
            summary: _,
            head,
            tail,
            section,
            jsonpath,
            max_bytes,
            json,
        } => {
            let mode = if let Some(n) = head {
                ReadMode::Head(n)
            } else if let Some(n) = tail {
                ReadMode::Tail(n)
            } else if let Some(sec) = section {
                ReadMode::Section(sec)
            } else if let Some(path) = jsonpath {
                ReadMode::JsonPath(path)
            } else {
                ReadMode::Summary
            };
            let r = store.read_handle(&handle, mode, max_bytes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                println!("{}", r.content);
            }
            Ok(0)
        }
    }
}
