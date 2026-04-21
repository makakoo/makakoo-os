//! `makakoo session <sub>` — CLI surface for the JSONL session tree.
//!
//! v0.2 Phase G.2 + G.4 + G.5. Every subcommand checks the
//! `kernel.session_tree` feature flag before touching anything. With
//! the flag off, we emit a one-line hint and exit 2 so CI pipelines
//! can distinguish "feature disabled" from "actual error" (exit 1).

use std::fs;
use std::io::Write;

use anyhow::{bail, Context};
use chrono::Utc;

use makakoo_core::kernel_config::KernelConfig;
use makakoo_core::session::export::{to_html, to_json, to_markdown};
use makakoo_core::session::{
    list_sessions, sessions_root,
    tree::{fork as session_fork, rewind_to_label, Entry, SessionTree},
};

use crate::cli::SessionCmd;
use crate::context::CliContext;

/// Entry point for `makakoo session`. Feature-flag gate runs first.
pub async fn run(ctx: &CliContext, cmd: SessionCmd) -> anyhow::Result<i32> {
    if !KernelConfig::load().session_tree_enabled() {
        eprintln!(
            "session tree is disabled — set `kernel.session_tree = true` \
             in {}/config/kernel.toml to enable",
            ctx.home().display(),
        );
        return Ok(2);
    }

    match cmd {
        SessionCmd::List { json } => list(ctx, json),
        SessionCmd::Show { id, json } => show(ctx, &id, json),
        SessionCmd::Fork { source, from, new_id } => fork_cmd(ctx, &source, &from, new_id),
        SessionCmd::Label { id, name } => label(ctx, &id, &name),
        SessionCmd::Rewind { id, label } => rewind(ctx, &id, &label),
        SessionCmd::Export { id, format, out } => export(ctx, &id, &format, out),
    }
}

fn list(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    let ids = list_sessions(ctx.home())
        .with_context(|| format!("reading {}", sessions_root(ctx.home()).display()))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&ids)?);
        return Ok(0);
    }

    if ids.is_empty() {
        println!("(no sessions)");
        return Ok(0);
    }
    for id in ids {
        println!("{}", id);
    }
    Ok(0)
}

fn open_tree(ctx: &CliContext, id: &str) -> anyhow::Result<SessionTree> {
    let tree = SessionTree::new(sessions_root(ctx.home()), id)
        .with_context(|| format!("opening session {id}"))?;
    if !tree.exists() {
        bail!("session {id} not found at {}", tree.path().display());
    }
    Ok(tree)
}

fn show(ctx: &CliContext, id: &str, json: bool) -> anyhow::Result<i32> {
    let tree = open_tree(ctx, id)?;
    let entries = tree
        .load()
        .with_context(|| format!("loading session {id}"))?;

    if json {
        println!("{}", to_json(&entries));
    } else {
        print!("{}", to_markdown(&entries));
    }
    Ok(0)
}

fn fork_cmd(
    ctx: &CliContext,
    source_id: &str,
    from_entry: &str,
    new_id: Option<String>,
) -> anyhow::Result<i32> {
    let source = open_tree(ctx, source_id)?;
    let resolved_new_id = new_id.unwrap_or_else(|| {
        format!("{source_id}-fork-{}", Utc::now().format("%Y%m%dT%H%M%S"))
    });
    let new = session_fork(&source, resolved_new_id.clone(), from_entry)
        .with_context(|| format!("forking session {source_id} at {from_entry}"))?;
    println!("{}", new.id());
    eprintln!("forked to {}", new.path().display());
    Ok(0)
}

fn label(ctx: &CliContext, id: &str, name: &str) -> anyhow::Result<i32> {
    let tree = open_tree(ctx, id)?;
    let entries = tree.load()?;
    let last = entries
        .last()
        .ok_or_else(|| anyhow::anyhow!("session {id} has no entries — cannot label"))?;

    let label_id = format!("label-{}", Utc::now().format("%Y%m%dT%H%M%S%.3f"));
    let entry = Entry::Label {
        id: label_id.clone(),
        parent_id: last.id().to_string(),
        name: name.to_string(),
        ts: Utc::now(),
    };
    tree.append(&entry)
        .with_context(|| format!("writing label {name} to session {id}"))?;
    println!("{label_id}");
    Ok(0)
}

fn rewind(ctx: &CliContext, id: &str, label_name: &str) -> anyhow::Result<i32> {
    let tree = open_tree(ctx, id)?;
    let kept = rewind_to_label(&tree, label_name)
        .with_context(|| format!("rewinding session {id} to label {label_name}"))?;
    println!("{} entries retained", kept);
    Ok(0)
}

fn export(
    ctx: &CliContext,
    id: &str,
    format: &str,
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<i32> {
    let tree = open_tree(ctx, id)?;
    let entries = tree.load()?;
    let body = match format {
        "md" | "markdown" => to_markdown(&entries),
        "html" => to_html(&entries),
        "json" => to_json(&entries),
        other => bail!("unknown --format {other:?} (accepted: markdown, html, json)"),
    };
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("creating {}", parent.display())
                    })?;
                }
            }
            let tmp = path.with_extension(format!(
                "{}.tmp",
                path.extension().and_then(|e| e.to_str()).unwrap_or("out"),
            ));
            fs::write(&tmp, body.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            fs::rename(&tmp, &path)
                .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(body.as_bytes())?;
        }
    }
    Ok(0)
}
