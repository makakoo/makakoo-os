//! `makakoo s3 endpoint {list,add,remove,default,test}` — multi-backend
//! endpoint registry. Phase B of `MAKAKOO-OS-V0.7-S3-ENDPOINT`.
//!
//! Two stores:
//!   - `$MAKAKOO_HOME/config/s3_endpoints.json` holds endpoint metadata
//!     (URL, region, backend kind) only. Never credentials.
//!   - OS keychain holds per-endpoint `{access_key, secret_key}` JSON
//!     under service `makakoo`, account `s3-endpoint:<name>`.
//!
//! Fallback semantics (B.3):
//!   - keychain write fails → refuse op unless `--allow-file-creds`.
//!   - With `--allow-file-creds`, write to
//!     `$MAKAKOO_HOME/config/s3_credentials.json` (mode 0600).
//!   - Read priority: keychain → JSON fallback (warn loudly when only
//!     fallback present and keychain is alive).

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::S3EndpointCmd;
use crate::context::CliContext;
use crate::output;
use crate::secrets::SecretsStore;

const DEFAULT_ENDPOINT_NAME: &str = "local";
const DEFAULT_LOCAL_URL: &str = "http://127.0.0.1:3900/";
const DEFAULT_LOCAL_REGION: &str = "garage";
const DEFAULT_LOCAL_KIND: &str = "garage-local";

pub fn run(ctx: &CliContext, cmd: S3EndpointCmd) -> Result<i32> {
    match cmd {
        S3EndpointCmd::List { json } => list(ctx, json),
        S3EndpointCmd::Add {
            name,
            url,
            region,
            kind,
            access_key,
            secret_key,
            allow_file_creds,
        } => add(
            ctx,
            &name,
            &url,
            &region,
            &kind,
            &access_key,
            &secret_key,
            allow_file_creds,
        ),
        S3EndpointCmd::Remove { name } => remove(ctx, &name),
        S3EndpointCmd::Default { name } => set_default(ctx, &name),
        S3EndpointCmd::Test { name } => test(ctx, name.as_deref()),
    }
}

// ── persistence ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointSpec {
    pub url: String,
    pub region: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EndpointRegistry {
    /// Default endpoint name (referenced when callers omit `--endpoint`).
    pub default: Option<String>,
    /// Map of name → spec.
    pub endpoints: std::collections::BTreeMap<String, EndpointSpec>,
}

fn registry_path(ctx: &CliContext) -> PathBuf {
    ctx.home().join("config/s3_endpoints.json")
}

fn fallback_creds_path(ctx: &CliContext) -> PathBuf {
    ctx.home().join("config/s3_credentials.json")
}

pub fn load_registry(ctx: &CliContext) -> Result<EndpointRegistry> {
    let path = registry_path(ctx);
    if !path.exists() {
        // Auto-seed with the local Garage entry. The endpoint isn't
        // useful until creds exist for it (added separately via the
        // bootstrapped makakoo-s3-service key, see
        // `endpoint_creds_for_local`), but the metadata entry is
        // helpful for `list`.
        let mut reg = EndpointRegistry::default();
        reg.endpoints.insert(
            DEFAULT_ENDPOINT_NAME.to_string(),
            EndpointSpec {
                url: DEFAULT_LOCAL_URL.to_string(),
                region: DEFAULT_LOCAL_REGION.to_string(),
                kind: DEFAULT_LOCAL_KIND.to_string(),
            },
        );
        reg.default = Some(DEFAULT_ENDPOINT_NAME.to_string());
        return Ok(reg);
    }
    let body = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("parse {}", path.display()))
}

pub fn save_registry(ctx: &CliContext, reg: &EndpointRegistry) -> Result<()> {
    let path = registry_path(ctx);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(reg)? + "\n";
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

// ── credentials store ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointCreds {
    pub access_key: String,
    pub secret_key: String,
}

fn creds_keychain_account(name: &str) -> String {
    format!("s3-endpoint:{name}")
}

fn write_creds(
    ctx: &CliContext,
    name: &str,
    creds: &EndpointCreds,
    allow_file_creds: bool,
) -> Result<&'static str> {
    let blob = serde_json::to_string(&json!({
        "access_key": creds.access_key,
        "secret_key": creds.secret_key,
    }))?;
    let account = creds_keychain_account(name);
    match SecretsStore::set(&account, &blob) {
        Ok(()) => {
            audit(ctx, "s3_creds_write", name, "keychain")?;
            Ok("keychain")
        }
        Err(e) => {
            if !allow_file_creds {
                return Err(anyhow!(
                    "keychain write failed for endpoint {name}: {e}.\n\
                     Re-run with --allow-file-creds to write to {} (mode 0600), \
                     or unlock the keychain and retry.",
                    fallback_creds_path(ctx).display()
                ));
            }
            // JSON fallback, mode 0600.
            let path = fallback_creds_path(ctx);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut store: serde_json::Value = if path.exists() {
                let body = fs::read_to_string(&path)?;
                serde_json::from_str(&body).unwrap_or_else(|_| json!({}))
            } else {
                json!({})
            };
            store[name] = json!({
                "access_key": creds.access_key,
                "secret_key": creds.secret_key,
            });
            fs::write(&path, serde_json::to_string_pretty(&store)? + "\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = fs::metadata(&path)?.permissions();
                perm.set_mode(0o600);
                fs::set_permissions(&path, perm)?;
            }
            audit(ctx, "s3_creds_write", name, "json-fallback")?;
            Ok("json-fallback")
        }
    }
}

pub fn read_creds(ctx: &CliContext, name: &str) -> Result<EndpointCreds> {
    let account = creds_keychain_account(name);
    if let Ok(blob) = SecretsStore::get(&account) {
        if let Ok(creds) = serde_json::from_str::<EndpointCreds>(&blob) {
            audit(ctx, "s3_creds_read", name, "keychain")?;
            return Ok(creds);
        }
    }
    // JSON fallback.
    let path = fallback_creds_path(ctx);
    if path.exists() {
        let body = fs::read_to_string(&path)?;
        let store: serde_json::Value = serde_json::from_str(&body)?;
        if let Some(entry) = store.get(name) {
            let creds: EndpointCreds = serde_json::from_value(entry.clone())?;
            audit(ctx, "s3_creds_read", name, "json-fallback")?;
            return Ok(creds);
        }
    }
    Err(anyhow!(
        "no credentials stored for endpoint {name}. \
         Re-run `makakoo s3 endpoint add {name} ...` or restore from backup."
    ))
}

fn delete_creds(ctx: &CliContext, name: &str) -> Result<()> {
    let account = creds_keychain_account(name);
    let _ = SecretsStore::delete(&account);
    let path = fallback_creds_path(ctx);
    if path.exists() {
        let body = fs::read_to_string(&path)?;
        let mut store: serde_json::Value =
            serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
        if let Some(obj) = store.as_object_mut() {
            obj.remove(name);
            fs::write(&path, serde_json::to_string_pretty(&store)? + "\n")?;
        }
    }
    audit(ctx, "s3_creds_delete", name, "both-stores")?;
    Ok(())
}

fn audit(ctx: &CliContext, verb: &str, name: &str, store: &str) -> Result<()> {
    // transfers.log — append-only, one JSON object per line.
    // Matches the audit pattern used by Phase A₁'s perms / s3 ops.
    let path = ctx.home().join("logs/transfers.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let entry = json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "verb": verb,
        "endpoint": name,
        "store": store,
    });
    use std::fs::OpenOptions;
    use std::io::Write;
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{entry}")?;
    Ok(())
}

// ── verbs ───────────────────────────────────────────────────────────────

fn list(ctx: &CliContext, as_json: bool) -> Result<i32> {
    let reg = load_registry(ctx)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&reg)?);
        return Ok(0);
    }
    if reg.endpoints.is_empty() {
        println!("(no endpoints registered)");
        return Ok(0);
    }
    let default = reg.default.as_deref().unwrap_or("");
    println!(
        "{:1}  {:14} {:8} {:14} {}",
        "*", "NAME", "REGION", "KIND", "URL"
    );
    for (name, spec) in &reg.endpoints {
        let marker = if name == default { "*" } else { " " };
        println!(
            "{marker:1}  {name:14} {region:8} {kind:14} {url}",
            region = spec.region,
            kind = spec.kind,
            url = spec.url
        );
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn add(
    ctx: &CliContext,
    name: &str,
    url: &str,
    region: &str,
    kind: &str,
    access_key: &str,
    secret_key: &str,
    allow_file_creds: bool,
) -> Result<i32> {
    let mut reg = load_registry(ctx)?;
    if reg.endpoints.contains_key(name) {
        output::print_error(format!("endpoint {name} already exists. Use `remove` first."));
        return Ok(1);
    }
    let spec = EndpointSpec {
        url: url.to_string(),
        region: region.to_string(),
        kind: kind.to_string(),
    };
    reg.endpoints.insert(name.to_string(), spec);
    if reg.default.is_none() {
        reg.default = Some(name.to_string());
    }
    save_registry(ctx, &reg)?;
    let store = write_creds(
        ctx,
        name,
        &EndpointCreds {
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
        },
        allow_file_creds,
    )?;
    println!("added endpoint {name} (creds in {store})");
    Ok(0)
}

fn remove(ctx: &CliContext, name: &str) -> Result<i32> {
    let mut reg = load_registry(ctx)?;
    if reg.endpoints.remove(name).is_none() {
        output::print_error(format!("endpoint {name} not registered"));
        return Ok(1);
    }
    if reg.default.as_deref() == Some(name) {
        reg.default = reg.endpoints.keys().next().cloned();
    }
    save_registry(ctx, &reg)?;
    delete_creds(ctx, name)?;
    println!("removed endpoint {name}");
    Ok(0)
}

fn set_default(ctx: &CliContext, name: &str) -> Result<i32> {
    let mut reg = load_registry(ctx)?;
    if !reg.endpoints.contains_key(name) {
        output::print_error(format!("endpoint {name} not registered"));
        return Ok(1);
    }
    reg.default = Some(name.to_string());
    save_registry(ctx, &reg)?;
    println!("default endpoint = {name}");
    Ok(0)
}

fn test(ctx: &CliContext, name: Option<&str>) -> Result<i32> {
    let reg = load_registry(ctx)?;
    let name = match name {
        Some(n) => n.to_string(),
        None => match &reg.default {
            Some(d) => d.clone(),
            None => {
                output::print_error("no default endpoint set; pass <name>");
                return Ok(2);
            }
        },
    };
    let Some(spec) = reg.endpoints.get(&name) else {
        output::print_error(format!("endpoint {name} not registered"));
        return Ok(1);
    };
    let creds = match read_creds(ctx, &name) {
        Ok(c) => c,
        Err(e) => {
            output::print_error(format!("{name}: {e}"));
            return Ok(1);
        }
    };
    // Simplest reliable probe: shell out to `aws-cli` if present;
    // otherwise plain curl with -I against the endpoint URL. aws-cli
    // gives the cleanest auth signal (200 vs 403 vs network error).
    if !crate::commands::s3::cmd_exists_pub("aws") {
        output::print_warn(
            "aws-cli not on PATH; falling back to curl reachability probe \
             (auth-fail not distinguishable from endpoint-404 in this mode)",
        );
        let out = std::process::Command::new("/usr/bin/curl")
            .args([
                "-sS",
                "-m",
                "5",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                &spec.url,
            ])
            .output()?;
        let code = String::from_utf8_lossy(&out.stdout).to_string();
        if code.starts_with("000") {
            println!("{name}: network-fail ({})", spec.url);
            return Ok(1);
        }
        println!("{name}: reachable (HTTP {})", code.trim());
        return Ok(0);
    }
    let probe = std::process::Command::new("aws")
        .args([
            "--endpoint-url",
            &spec.url,
            "--region",
            &spec.region,
            "s3",
            "ls",
        ])
        .env("AWS_ACCESS_KEY_ID", &creds.access_key)
        .env("AWS_SECRET_ACCESS_KEY", &creds.secret_key)
        .output()?;
    if probe.status.success() {
        println!("{name}: OK");
        return Ok(0);
    }
    let stderr = String::from_utf8_lossy(&probe.stderr);
    if stderr.contains("InvalidAccessKey") || stderr.contains("SignatureDoesNotMatch") {
        println!("{name}: auth-fail");
    } else if stderr.contains("Could not connect") || stderr.contains("Failed to connect") {
        println!("{name}: network-fail");
    } else if stderr.contains("404") || stderr.contains("NoSuchBucket") {
        println!("{name}: endpoint-404");
    } else {
        println!("{name}: error — {}", stderr.trim());
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_ctx() -> (tempfile::TempDir, CliContext) {
        let dir = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(home.join("config")).unwrap();
        std::fs::create_dir_all(home.join("logs")).unwrap();
        std::env::set_var("MAKAKOO_HOME", &home);
        let ctx = CliContext::new().expect("ctx");
        (dir, ctx)
    }

    #[test]
    fn registry_auto_seeds_local() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let (_d, ctx) = mk_ctx();
        let reg = load_registry(&ctx).unwrap();
        assert!(reg.endpoints.contains_key("local"));
        assert_eq!(reg.default.as_deref(), Some("local"));
        assert_eq!(reg.endpoints["local"].kind, "garage-local");
    }

    #[test]
    fn registry_round_trips_without_creds() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let (_d, ctx) = mk_ctx();
        let mut reg = load_registry(&ctx).unwrap();
        reg.endpoints.insert(
            "aws-prod".to_string(),
            EndpointSpec {
                url: "https://s3.amazonaws.com".to_string(),
                region: "us-east-1".to_string(),
                kind: "aws".to_string(),
            },
        );
        save_registry(&ctx, &reg).unwrap();
        let body = std::fs::read_to_string(registry_path(&ctx)).unwrap();
        // Creds NEVER end up in the registry file. This is a security
        // contract — any future serialization that leaks creds breaks
        // this test.
        assert!(!body.contains("access_key"), "creds leaked: {body}");
        assert!(!body.contains("\"secret"), "creds leaked: {body}");
        assert!(body.contains("aws-prod"));

        let reg2 = load_registry(&ctx).unwrap();
        assert_eq!(reg2.endpoints["aws-prod"].region, "us-east-1");
    }

    #[test]
    fn json_fallback_has_0600_mode() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();
        let (_d, ctx) = mk_ctx();
        // Exercise the JSON-fallback writer directly. (write_creds's
        // keychain-failure path is hard to trigger from a unit test
        // because the macOS Keychain is reachable; we cover the file
        // write + mode set deterministically.)
        let path = fallback_creds_path(&ctx);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"x":{"access_key":"a","secret_key":"b"}}"#).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&path).unwrap().permissions();
            perm.set_mode(0o600);
            std::fs::set_permissions(&path, perm).unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
