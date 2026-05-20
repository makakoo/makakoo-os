use makakoo_core::agent_session::{
    self, AgentSessionRole, AgentSessionStatus, AgentSessionStore, ReadMode,
};
use serde_json::json;

use crate::cli::AgentSessionCmd;
use crate::context::CliContext;
use crate::output;

pub fn run(ctx: &CliContext, cmd: AgentSessionCmd) -> anyhow::Result<i32> {
    let store = store(ctx)?;
    match cmd {
        AgentSessionCmd::Open {
            name,
            role,
            task,
            workspace,
            model,
            json,
        } => {
            let role = AgentSessionRole::parse(&role)?;
            let session =
                store.open_session(&name, role, &task, &workspace, model.as_deref(), json!({}))?;
            print_session(&session, json);
            Ok(0)
        }
        AgentSessionCmd::List {
            status,
            include_closed,
            json,
        } => {
            let status = status
                .as_deref()
                .map(AgentSessionStatus::parse)
                .transpose()?;
            let rows = store.list(status, include_closed)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if rows.is_empty() {
                    println!("(no agent sessions)");
                }
                for s in rows {
                    println!(
                        "{}\t{}\t{}\t{}",
                        s.id,
                        s.status.as_str(),
                        s.role.as_str(),
                        s.name
                    );
                }
            }
            Ok(0)
        }
        AgentSessionCmd::Status { name_or_id, json } => {
            let s = store.get(&name_or_id)?;
            print_session(&s, json);
            Ok(0)
        }
        AgentSessionCmd::Eval {
            name_or_id,
            wait: _,
            timeout: _,
            message,
            json,
        } => {
            let mut s = store.get(&name_or_id)?;
            if matches!(
                s.status,
                AgentSessionStatus::Queued | AgentSessionStatus::Running
            ) {
                if s.status == AgentSessionStatus::Queued {
                    store.mark_running(&s.id)?;
                }
                let task = message.as_deref().unwrap_or(&s.assignment);
                let result = format!(
                    "SUMMARY:\nCompleted sync session {}.\nCHANGES:\nNone.\nEVIDENCE:\nsession_id={}\ntask={}\nRISKS:\nSync CLI v1; detached daemon workers are v1.1.\nBLOCKERS:\nNone.\n",
                    s.name, s.id, task.chars().take(300).collect::<String>()
                );
                let handle = store.publish_artifact(
                    Some(&s.id),
                    "result",
                    "agent-session",
                    &result,
                    "text/plain",
                    true,
                )?;
                store.complete(
                    &s.id,
                    &format!("Completed sync session {}.", s.name),
                    Some(&handle),
                )?;
                s = store.get(&s.id)?;
            }
            print_session(&s, json);
            Ok(0)
        }
        AgentSessionCmd::Read {
            name_or_id,
            section,
            head,
            tail,
            json,
        } => {
            let s = store.get(&name_or_id)?;
            let Some(handle) = s
                .result_handle
                .as_deref()
                .or(s.transcript_handle.as_deref())
            else {
                output::print_error(format!("session has no result handle yet: {}", s.id));
                return Ok(4);
            };
            let mode = if let Some(sec) = section {
                ReadMode::Section(sec)
            } else if let Some(n) = head {
                ReadMode::Head(n)
            } else if let Some(n) = tail {
                ReadMode::Tail(n)
            } else {
                ReadMode::Summary
            };
            let r = store.read_handle(handle, mode, 8192)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                println!("{}", r.content);
            }
            Ok(0)
        }
        AgentSessionCmd::Close {
            name_or_id,
            cancel,
            json,
        } => {
            let s = store.close(&name_or_id, cancel)?;
            print_session(&s, json);
            Ok(0)
        }
        AgentSessionCmd::Gate {
            name_or_id,
            name,
            cwd,
            cmd,
            json,
        } => {
            let g = store.run_gate(&name_or_id, &name, &cwd, &cmd)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&g)?);
            } else {
                println!(
                    "{} {} exit={} log={}",
                    g.classification,
                    g.name,
                    g.exit_code,
                    g.log_artifact_id.unwrap_or_default()
                );
            }
            Ok(if g.exit_code == 0 { 0 } else { 1 })
        }
        AgentSessionCmd::Gates { name_or_id, json } => {
            let gates = store.gates(&name_or_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&gates)?);
            } else {
                for g in gates {
                    println!(
                        "{} {} exit={} log={}",
                        g.classification,
                        g.name,
                        g.exit_code,
                        g.log_artifact_id.unwrap_or_default()
                    );
                }
            }
            Ok(0)
        }
    }
}

fn store(ctx: &CliContext) -> anyhow::Result<AgentSessionStore> {
    let bus = ctx.event_bus().ok();
    Ok(AgentSessionStore::open(
        &agent_session::db_path(ctx.home()),
        bus,
    )?)
}

fn print_session(s: &makakoo_core::agent_session::AgentSession, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(s).unwrap());
    } else {
        println!(
            "{}\t{}\t{}\t{}",
            s.id,
            s.status.as_str(),
            s.role.as_str(),
            s.name
        );
        if let Some(h) = &s.result_handle {
            println!("result: {h}");
        }
        if !s.error.is_empty() {
            println!("error: {}", s.error);
        }
    }
}
