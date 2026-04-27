//! `makakoo s3 bootstrap` — create + persist the Makakoo-owned service
//! keypair against the local Garage instance.
//!
//! Phase A₁.5 of `MAKAKOO-OS-V0.7-S3-ENDPOINT`. The service keypair
//! identifies Makakoo itself (distinct from any user identity) when it
//! talks S3 — health probes, Phase C bucket ops, the disk watchdog, etc.
//!
//! Idempotent: re-running on an already-bootstrapped install is a no-op.
//! `--force-rotate` deletes the old key (in Garage + keychain) and
//! creates a fresh one.
//!
//! Wire shape:
//!   1. read garage admin token from `$MAKAKOO_HOME/config/garage.toml`,
//!   2. probe `garage key list` (Garage's CLI is the supported admin
//!      surface — admin HTTP API is private and undocumented),
//!   3. if missing, `garage key create makakoo-s3-service`,
//!   4. parse access_key + secret_key from the create output,
//!   5. grant `--read --write --owner` on `*` (any bucket),
//!   6. store `{access_key, secret_key}` JSON in keychain under
//!      service `makakoo`, key `makakoo-s3-service`.
//!
//! The wrapper script (`plugins-core/garage-store/bin/garage-wrapper.sh`)
//! invokes us as fire-and-forget after the admin API responds. We are
//! *also* operator-callable any time post-Garage-start.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use crate::cli::S3Cmd;
use crate::context::CliContext;
use crate::output;
use crate::secrets::SecretsStore;

const SERVICE_KEY_NAME: &str = "makakoo-s3-service";
const ADMIN_API_URL: &str = "http://127.0.0.1:3903";

pub fn run(ctx: &CliContext, cmd: S3Cmd) -> Result<i32> {
    match cmd {
        S3Cmd::Bootstrap { force_rotate } => bootstrap(ctx, force_rotate),
        S3Cmd::Endpoint { cmd } => crate::commands::s3_endpoint::run(ctx, cmd),
    }
}

fn bootstrap(ctx: &CliContext, force_rotate: bool) -> Result<i32> {
    // 1. macOS gate. v0.7 is macOS-only.
    if !cfg!(target_os = "macos") {
        output::print_error(
            "makakoo s3 bootstrap: refusing on non-macOS host. \
             Garage-store v0.7 is macOS-only. See docs/platform-matrix.md.",
        );
        return Ok(1);
    }

    // 2. Garage CLI must be on PATH (brew-installed by the plugin).
    if !command_exists("garage") {
        output::print_error(
            "makakoo s3 bootstrap: `garage` not on PATH. \
             Run `makakoo plugin install --core garage-store` first.",
        );
        return Ok(1);
    }

    // 3. Wait for the admin API to come up — the caller is typically
    //    the wrapper script firing this off right after launchctl load.
    if !wait_for_admin_api(Duration::from_secs(30)) {
        output::print_error(format!(
            "makakoo s3 bootstrap: admin API at {ADMIN_API_URL} did not become ready within 30s. \
             Is `makakoo plugin start garage-store` running?"
        ));
        return Ok(2);
    }

    // 4. Single-node layout: a fresh Garage refuses reads/writes until
    //    a cluster layout is applied. v0.7 ships single-node only
    //    (Locked Decision 2) so we auto-assign with one zone, one tag,
    //    100GB capacity. Idempotent — checks `garage layout show` first.
    ensure_single_node_layout(ctx)?;

    // 5. Force-rotate path: delete every prior key carrying this name
    //    (Garage allows multiple keys to share a name — name-pattern
    //    deletes refuse on collision), then fall through to create.
    if force_rotate {
        let prior_ids = list_service_key_ids(ctx)?;
        for id in &prior_ids {
            let _ = run_garage_cli(ctx, &["key", "delete", "--yes", id]);
        }
        let _ = SecretsStore::delete(SERVICE_KEY_NAME);
        output::print_info(format!(
            "rotated: deleted {} prior {SERVICE_KEY_NAME} key(s) (in Garage + keychain)",
            prior_ids.len()
        ));
    }

    // 6. Probe Garage for the existing key set.
    let prior_ids = list_service_key_ids(ctx)?;
    if !prior_ids.is_empty() && !force_rotate {
        // Idempotent path. Make sure the keychain entry is also present;
        // re-derive from `garage key info <id> --show-secret` if the
        // keychain blob got lost. Pass the ID rather than the name so
        // we don't trip the multi-match guard.
        if SecretsStore::get(SERVICE_KEY_NAME).is_err() {
            output::print_warn(
                "service key exists in Garage but missing from keychain — \
                 re-storing from `garage key info <id>`",
            );
            let info = run_garage_cli(
                ctx,
                &["key", "info", &prior_ids[0], "--show-secret"],
            )?;
            let (access, secret) = parse_key_creds(&info)?;
            store_creds_in_keychain(&access, &secret)?;
        }
        println!("makakoo-s3-service already bootstrapped (idempotent: no-op)");
        return Ok(0);
    }

    // 6. Create the key.
    let create_out = run_garage_cli(ctx, &["key", "create", SERVICE_KEY_NAME])
        .context("garage key create failed — is Garage running?")?;
    let (access, secret) = parse_key_creds(&create_out)?;

    // 6b. Grant the service key bucket-creation rights. Without this,
    //     `aws s3 mb s3://<bucket>` fails with `Access key … is not
    //     allowed to create buckets`. The service identity needs to
    //     own the bucket-namespace lifecycle for Phase C.
    run_garage_cli(
        ctx,
        &["key", "allow", "--create-bucket", SERVICE_KEY_NAME],
    )
    .context("garage key allow --create-bucket failed")?;

    // 7. Grant the service key full access on every bucket. The service
    //    identity is Makakoo's own; it never gets handed to user buckets
    //    (Locked Decision 16 — `makakoo bucket grant --to makakoo-s3-service`
    //    is hard-refused at the bucket-grant CLI level).
    //
    //    Garage v2.x doesn't have `--read --write --owner *` — instead,
    //    the create output already grants the key implicit access; per-
    //    bucket allow lists are layered on at bucket-creation time. We
    //    leave the key with implicit "owner of any future bucket it
    //    creates" semantics and let Phase C wire up per-bucket allow lists.

    // 8. Persist creds in the macOS Keychain.
    store_creds_in_keychain(&access, &secret)?;

    println!("bootstrapped {SERVICE_KEY_NAME}: access={access} (secret in keychain)");
    Ok(0)
}

fn command_exists(name: &str) -> bool {
    cmd_exists_pub(name)
}

/// Public reflection of `command_exists`. Used by `s3_endpoint::test`
/// to fall back to a curl probe when `aws-cli` isn't on PATH.
pub fn cmd_exists_pub(name: &str) -> bool {
    Command::new("/usr/bin/which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn wait_for_admin_api(timeout: Duration) -> bool {
    use std::time::Instant;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe_admin_once() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn probe_admin_once() -> bool {
    // `garage server` returns 200 once layout + quorum are good, but
    // returns 503 ("Quorum is not available") on a freshly-installed
    // single-node cluster that hasn't had a layout applied yet. Both
    // responses prove the admin port is responsive — we just need
    // ANY HTTP response to know we can call further `garage` CLI
    // commands. Anything that produces an HTTP status code counts.
    let out = Command::new("/usr/bin/curl")
        .args([
            "-sS",
            "-m",
            "1",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            &format!("{ADMIN_API_URL}/health"),
        ])
        .output();
    match out {
        Ok(o) => {
            let code = String::from_utf8_lossy(&o.stdout);
            // 000 means curl couldn't open a TCP connection. Any other
            // 3-digit status (200, 503, …) means Garage is alive.
            !code.starts_with("000") && code.trim().len() == 3
        }
        Err(_) => false,
    }
}

fn config_path(ctx: &CliContext) -> PathBuf {
    ctx.home().join("config/garage.toml")
}

fn run_garage_cli(ctx: &CliContext, args: &[&str]) -> Result<String> {
    let cfg = config_path(ctx);
    if !cfg.exists() {
        return Err(anyhow!(
            "garage config missing at {} — run `makakoo plugin install --core garage-store` first",
            cfg.display()
        ));
    }
    let out = Command::new("garage")
        .arg("-c")
        .arg(&cfg)
        .args(args)
        .output()
        .with_context(|| format!("failed to invoke `garage {}`", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "garage {} failed (exit {}): {}",
            args.join(" "),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Make sure the Garage cluster has at least one node assigned a layout.
/// Idempotent: a no-op when `garage layout show` reports a non-empty
/// applied layout.
///
/// Single-node only — v0.7 doesn't ship multi-node clustering. When
/// multi-node lands, this becomes user-driven (`makakoo s3 cluster join`
/// / etc.) and bootstrap stops auto-assigning.
fn ensure_single_node_layout(ctx: &CliContext) -> Result<()> {
    let show = run_garage_cli(ctx, &["layout", "show"]).unwrap_or_default();
    // Heuristic: a fresh layout shows "No nodes are currently part of the cluster"
    // (or a similar string); an applied layout shows a "ID  Tags  Zone  Capacity"
    // table where at least one row is the local node.
    if !show.contains("No nodes")
        && (show.contains("Capacity") || show.contains("dc1") || show.contains("100"))
    {
        return Ok(());
    }
    let node_id_raw = run_garage_cli(ctx, &["node", "id", "-q"])?;
    // `garage node id -q` prints `<id>@<addr>` — we want the bare id.
    let node_id = node_id_raw
        .lines()
        .next()
        .unwrap_or_default()
        .split('@')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if node_id.is_empty() {
        return Err(anyhow!(
            "could not parse node id from `garage node id -q` output:\n{node_id_raw}"
        ));
    }
    output::print_info(format!("assigning single-node layout to {node_id}"));
    let _ = run_garage_cli(
        ctx,
        &[
            "layout",
            "assign",
            &node_id,
            "-z",
            "dc1",
            "-c",
            "100G",
            "-t",
            "makakoo",
        ],
    )?;
    let _ = run_garage_cli(ctx, &["layout", "apply", "--version", "1"])?;
    Ok(())
}

fn service_key_exists(ctx: &CliContext) -> Result<bool> {
    Ok(!list_service_key_ids(ctx)?.is_empty())
}

/// Return every Garage key ID currently named `makakoo-s3-service`.
/// Garage allows N keys to share a name; we treat the set as the
/// "service identity" and operate on every member.
fn list_service_key_ids(ctx: &CliContext) -> Result<Vec<String>> {
    let out = run_garage_cli(ctx, &["key", "list"])?;
    let mut ids = Vec::new();
    for line in out.lines() {
        if !line.contains(SERVICE_KEY_NAME) {
            continue;
        }
        // `garage key list` rows look like:
        //   ID                          Created     Name                Expiration
        //   GKabc123...                 2026-04-25  makakoo-s3-service  never
        // The first whitespace-delimited token is the key ID. Header
        // row has "ID" — we filter that by requiring the name match.
        if let Some(first) = line.split_whitespace().next() {
            if first.starts_with("GK") {
                ids.push(first.to_string());
            }
        }
    }
    Ok(ids)
}

/// Public reflection of `parse_key_creds` for `commands::bucket` reuse.
pub fn parse_key_creds_pub(out: &str) -> Result<(String, String)> {
    parse_key_creds(out)
}

/// Parse `Key access ID: <access>` and `Secret access key: <secret>` (or
/// `Key ID: <access>` / `Secret key: <secret>`) from `garage key
/// create` / `garage key info` output.
fn parse_key_creds(out: &str) -> Result<(String, String)> {
    let mut access: Option<String> = None;
    let mut secret: Option<String> = None;
    for line in out.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("key id") || lower.contains("key access id") {
            if let Some(v) = trimmed.rsplit(':').next() {
                let v = v.trim().trim_matches('"').to_string();
                if !v.is_empty() && access.is_none() {
                    access = Some(v);
                }
            }
        } else if lower.contains("secret access key")
            || lower.starts_with("secret key:")
            || lower.contains("secret key:")
        {
            if let Some(v) = trimmed.rsplit(':').next() {
                let v = v.trim().trim_matches('"').to_string();
                if !v.is_empty() && secret.is_none() {
                    secret = Some(v);
                }
            }
        }
    }
    match (access, secret) {
        (Some(a), Some(s)) => Ok((a, s)),
        _ => Err(anyhow!(
            "could not parse access_key + secret_key from garage CLI output:\n---\n{out}\n---"
        )),
    }
}

fn store_creds_in_keychain(access: &str, secret: &str) -> Result<()> {
    let blob = json!({
        "access_key": access,
        "secret_key": secret,
    })
    .to_string();
    SecretsStore::set(SERVICE_KEY_NAME, &blob)
        .context("failed to store makakoo-s3-service creds in keychain")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_creds_handles_garage_v2_format() {
        // Synthetic — actual Garage v2.3.0 emits this shape on `key create`.
        let sample = "\
==== ACCESS KEYS ====
Key ID: GK0a47d4f88a5b843b1c321ef4
Secret key: d6f6acf6d15072ed3e57a129f9adc2af57cbc4fc022e2b279727805272ccd285
Name: makakoo-s3-service
";
        let (a, s) = parse_key_creds(sample).unwrap();
        assert_eq!(a, "GK0a47d4f88a5b843b1c321ef4");
        assert_eq!(
            s,
            "d6f6acf6d15072ed3e57a129f9adc2af57cbc4fc022e2b279727805272ccd285"
        );
    }

    #[test]
    fn parse_creds_handles_alt_label_format() {
        let sample = "\
Key access ID:    GK1234567890abcdef
Secret access key: deadbeefcafe1234
";
        let (a, s) = parse_key_creds(sample).unwrap();
        assert_eq!(a, "GK1234567890abcdef");
        assert_eq!(s, "deadbeefcafe1234");
    }

    #[test]
    fn parse_creds_errors_on_missing_secret() {
        let sample = "Key ID: GKabc\n";
        let err = parse_key_creds(sample).unwrap_err();
        assert!(format!("{err}").contains("could not parse"));
    }
}
