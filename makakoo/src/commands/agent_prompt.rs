//! Send one prompt to a running DeepSeek Harness slot runtime.

use std::path::{Path, PathBuf};
use std::time::Duration;

use makakoo_core::agents::{AgentRuntimeEngine, AgentSlot};
use serde::Deserialize;

use crate::context::CliContext;

#[derive(Debug, Deserialize)]
struct RuntimeInfo {
    slot: String,
    engine: String,
    host: String,
    port: u16,
    pid: u32,
    token_file: PathBuf,
}

pub fn run(ctx: &CliContext, slot_id: &str, text: &str, session_id: &str) -> anyhow::Result<i32> {
    let project_dir = dsh_project_dir(ctx, slot_id)?;
    let endpoint = read_endpoint(&project_dir, slot_id)?;
    let token = std::fs::read_to_string(&endpoint.token_file).map_err(|e| {
        anyhow::anyhow!(
            "read runtime token {}: {}",
            endpoint.token_file.display(),
            e
        )
    })?;
    let request = serde_json::json!({"text": text, "session_id": session_id});
    let url = format!("http://127.0.0.1:{}/v1/run", endpoint.port);
    let response = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()?
                .post(url)
                .bearer_auth(token.trim())
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            let bytes = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, bytes))
        })
    })
    .map_err(|error| {
        anyhow::anyhow!(
            "slot '{}' is not responding at 127.0.0.1:{}: {}",
            slot_id,
            endpoint.port,
            error
        )
    })?;
    let (status, bytes) = response;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|body| {
                body.get("error")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        anyhow::bail!("agent runtime returned {}: {}", status, message);
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::anyhow!(
            "agent runtime returned {} with invalid JSON: {} ({})",
            status,
            error,
            String::from_utf8_lossy(&bytes)
        )
    })?;
    let answer = body
        .get("response")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("agent runtime response missing 'response'"))?;
    println!("{}", answer);
    Ok(0)
}

pub fn health(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let project_dir = dsh_project_dir(ctx, slot_id)?;
    let endpoint = read_endpoint(&project_dir, slot_id)?;
    let url = format!("http://127.0.0.1:{}/health", endpoint.port);
    let response = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()?
                .get(url)
                .send()
                .await
        })
    });
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            println!(
                "{}: not running (127.0.0.1:{}: {})",
                slot_id, endpoint.port, error
            );
            return Ok(1);
        }
    };
    if !response.status().is_success() {
        return Ok(1);
    }
    println!("{}: healthy (deepseek-harness)", slot_id);
    Ok(0)
}

fn dsh_project_dir(ctx: &CliContext, slot_id: &str) -> anyhow::Result<PathBuf> {
    let slot_path = makakoo_core::agents::checked_slot_path(ctx.home(), slot_id)?;
    let slot = AgentSlot::load_from_file(&slot_path)
        .map_err(|e| anyhow::anyhow!("agent slot '{}' load failed: {}", slot_id, e))?;
    let runtime = slot.runtime.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "slot '{}' uses the legacy gateway and has no runtime API",
            slot_id
        )
    })?;
    if runtime.engine != AgentRuntimeEngine::DeepseekHarness {
        anyhow::bail!("slot '{}' is not a DeepSeek Harness runtime", slot_id);
    }
    Ok(runtime.project_dir.clone())
}

fn read_endpoint(project_dir: &Path, expected_slot: &str) -> anyhow::Result<RuntimeInfo> {
    let path = project_dir.join("runtime.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "runtime metadata unavailable at {}: {} (is the slot started?)",
            path.display(),
            e
        )
    })?;
    let info: RuntimeInfo = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("invalid runtime metadata {}: {}", path.display(), e))?;
    if info.slot != expected_slot || info.engine != "deepseek-harness" {
        anyhow::bail!(
            "runtime metadata does not belong to slot '{}'",
            expected_slot
        );
    }
    if info.host != "127.0.0.1" || info.port == 0 {
        anyhow::bail!("runtime endpoint must be a non-zero loopback port");
    }
    if !makakoo_core::agents::process::pid_is_alive(info.pid) {
        anyhow::bail!(
            "slot '{}' is not running (stale runtime metadata for pid {})",
            expected_slot,
            info.pid
        );
    }
    let project = project_dir.canonicalize()?;
    let token = info.token_file.canonicalize()?;
    if !token.starts_with(&project) {
        anyhow::bail!("runtime token file escapes the generated project directory");
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_rejects_remote_host() {
        let tmp = tempfile::tempdir().unwrap();
        let token = tmp.path().join(".runtime-token");
        std::fs::write(&token, "secret").unwrap();
        std::fs::write(
            tmp.path().join("runtime.json"),
            serde_json::json!({
                "slot": "researcher",
                "engine": "deepseek-harness",
                "host": "0.0.0.0",
                "port": 9000,
                "pid": std::process::id(),
                "token_file": token,
            })
            .to_string(),
        )
        .unwrap();
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }

    #[test]
    fn endpoint_accepts_scoped_token_file() {
        let tmp = tempfile::tempdir().unwrap();
        let token = tmp.path().join(".runtime-token");
        std::fs::write(&token, "secret").unwrap();
        std::fs::write(
            tmp.path().join("runtime.json"),
            serde_json::json!({
                "slot": "researcher",
                "engine": "deepseek-harness",
                "host": "127.0.0.1",
                "port": 9000,
                "pid": std::process::id(),
                "token_file": token,
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(read_endpoint(tmp.path(), "researcher").unwrap().port, 9000);
    }

    #[test]
    fn endpoint_rejects_stale_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let token = tmp.path().join(".runtime-token");
        std::fs::write(&token, "secret").unwrap();
        std::fs::write(
            tmp.path().join("runtime.json"),
            serde_json::json!({
                "slot": "researcher",
                "engine": "deepseek-harness",
                "host": "127.0.0.1",
                "port": 9000,
                "pid": 0,
                "token_file": token,
            })
            .to_string(),
        )
        .unwrap();
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(error.to_string().contains("stale runtime metadata"));
    }
}
