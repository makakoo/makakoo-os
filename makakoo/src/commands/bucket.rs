//! `makakoo bucket …` — adapter wrapping the standalone `garagetytus`
//! binary (Phase D, GARAGETYTUS-V0.1).
//!
//! The bucket lifecycle business logic lives in
//! `github.com/traylinx/garagetytus`. Makakoo's role is the umbrella
//! UX: parse the same flags via `cli::BucketCmd`, then forward to
//! `garagetytus bucket <sub> [args]` with inherited stdio so
//! `--json` output and structured stderr stream verbatim through to
//! scripted callers.
//!
//! Per Q2 verdict (`development/sprints/queued/GARAGETYTUS-V0.1/
//! verdicts/Q2-VERDICT.md`, lope 2026-04-25, pi+codex PASS Option A):
//!
//!   * Inherited stdio (no capture) — `--json` flows through.
//!   * "garagetytus not found" → stderr install pointer + non-zero
//!     exit. **No silent embedded-Garage fallback.**
//!   * Daemon-not-running / port-collision / config-broken stay
//!     garagetytus's concerns to surface; Makakoo doesn't second-
//!     guess them.
//!
//! Adapter discovery probe (`adapter.toml.discovery.probe`) hits
//! `garagetytus capabilities --json`. Phase D.3 fallback message is
//! emitted from this module on `ErrorKind::NotFound` from the spawn.

use std::process::{Command, Stdio};

use anyhow::Result;

use crate::cli::BucketCmd;
use crate::context::CliContext;

const NOT_FOUND_MSG: &str =
    "garagetytus not found — install at https://garagetytus.dev";

pub async fn run(_ctx: &CliContext, cmd: BucketCmd) -> Result<i32> {
    let args = render_args(&cmd);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(spawn_garagetytus(&arg_refs))
}

fn spawn_garagetytus(args: &[&str]) -> i32 {
    let result = Command::new("garagetytus")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match result {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{}", NOT_FOUND_MSG);
            1
        }
        Err(e) => {
            eprintln!("makakoo bucket: failed to spawn garagetytus: {e}");
            1
        }
    }
}

fn render_args(cmd: &BucketCmd) -> Vec<String> {
    let mut out: Vec<String> = vec!["bucket".into()];
    match cmd {
        BucketCmd::Create {
            name,
            endpoint,
            ttl,
            quota,
            confirm_yes_really,
        } => {
            out.push("create".into());
            out.push(name.clone());
            if let Some(ep) = endpoint {
                out.push("--endpoint".into());
                out.push(ep.clone());
            }
            out.push("--ttl".into());
            out.push(ttl.clone());
            out.push("--quota".into());
            out.push(quota.clone());
            if *confirm_yes_really {
                out.push("--confirm-yes-really".into());
            }
        }
        BucketCmd::List { endpoint, json } => {
            out.push("list".into());
            if let Some(ep) = endpoint {
                out.push("--endpoint".into());
                out.push(ep.clone());
            }
            if *json {
                out.push("--json".into());
            }
        }
        BucketCmd::Info { name, json } => {
            out.push("info".into());
            out.push(name.clone());
            if *json {
                out.push("--json".into());
            }
        }
        BucketCmd::Grant {
            bucket,
            to,
            perms,
            ttl,
            confirm_yes_really,
            json,
        } => {
            out.push("grant".into());
            out.push(bucket.clone());
            out.push("--to".into());
            out.push(to.clone());
            out.push("--perms".into());
            out.push(perms.clone());
            out.push("--ttl".into());
            out.push(ttl.clone());
            if *confirm_yes_really {
                out.push("--confirm-yes-really".into());
            }
            if *json {
                out.push("--json".into());
            }
        }
        BucketCmd::Revoke { grant_id } => {
            out.push("revoke".into());
            out.push(grant_id.clone());
        }
        BucketCmd::Expire { dry_run } => {
            out.push("expire".into());
            if *dry_run {
                out.push("--dry-run".into());
            }
        }
        BucketCmd::DenyAll {
            name,
            ttl,
            confirm_yes_really,
        } => {
            out.push("deny-all".into());
            out.push(name.clone());
            out.push("--ttl".into());
            out.push(ttl.clone());
            if *confirm_yes_really {
                out.push("--confirm-yes-really".into());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_create_full_flags() {
        let cmd = BucketCmd::Create {
            name: "demo".into(),
            endpoint: Some("local".into()),
            ttl: "7d".into(),
            quota: "1G".into(),
            confirm_yes_really: false,
        };
        assert_eq!(
            render_args(&cmd),
            vec![
                "bucket", "create", "demo", "--endpoint", "local",
                "--ttl", "7d", "--quota", "1G",
            ]
        );
    }

    #[test]
    fn render_create_with_confirm() {
        let cmd = BucketCmd::Create {
            name: "demo".into(),
            endpoint: None,
            ttl: "permanent".into(),
            quota: "unlimited".into(),
            confirm_yes_really: true,
        };
        assert_eq!(
            render_args(&cmd),
            vec![
                "bucket", "create", "demo",
                "--ttl", "permanent", "--quota", "unlimited",
                "--confirm-yes-really",
            ]
        );
    }

    #[test]
    fn render_list_with_json() {
        let cmd = BucketCmd::List {
            endpoint: None,
            json: true,
        };
        assert_eq!(render_args(&cmd), vec!["bucket", "list", "--json"]);
    }

    #[test]
    fn render_grant_full() {
        let cmd = BucketCmd::Grant {
            bucket: "demo".into(),
            to: "external-app".into(),
            perms: "read,write".into(),
            ttl: "1h".into(),
            confirm_yes_really: false,
            json: true,
        };
        assert_eq!(
            render_args(&cmd),
            vec![
                "bucket", "grant", "demo",
                "--to", "external-app",
                "--perms", "read,write",
                "--ttl", "1h",
                "--json",
            ]
        );
    }

    #[test]
    fn render_revoke() {
        let cmd = BucketCmd::Revoke { grant_id: "g_20260425_abc".into() };
        assert_eq!(
            render_args(&cmd),
            vec!["bucket", "revoke", "g_20260425_abc"]
        );
    }

    #[test]
    fn render_expire_dry_run() {
        let cmd = BucketCmd::Expire { dry_run: true };
        assert_eq!(render_args(&cmd), vec!["bucket", "expire", "--dry-run"]);
    }

    #[test]
    fn render_deny_all() {
        let cmd = BucketCmd::DenyAll {
            name: "demo".into(),
            ttl: "1h".into(),
            confirm_yes_really: false,
        };
        assert_eq!(
            render_args(&cmd),
            vec!["bucket", "deny-all", "demo", "--ttl", "1h"]
        );
    }

    #[test]
    fn spawn_garagetytus_emits_install_hint_when_missing() {
        // This test cannot capture stderr without re-running ourselves
        // as a subprocess, so we just exercise the not-found path
        // returns the documented exit code. PATH is mutated to a
        // garagetytus-free directory so the spawn fails with NotFound.
        let path_backup = std::env::var_os("PATH");
        std::env::set_var("PATH", "/nonexistent");
        let code = spawn_garagetytus(&["bucket", "list"]);
        if let Some(p) = path_backup {
            std::env::set_var("PATH", p);
        } else {
            std::env::remove_var("PATH");
        }
        assert_eq!(code, 1);
    }
}
