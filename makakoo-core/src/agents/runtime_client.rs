//! Loopback client for a running DeepSeek Harness slot runtime.
//!
//! The generated `runner.mjs` binds an ephemeral loopback port, writes
//! `runtime.json` (mode 0600) next to itself, and authenticates every
//! `POST /v1/run` with the bearer token in `.runtime-token`. Two
//! callers need that endpoint: `makakoo agent prompt` (one-shot, from
//! the CLI) and the transport bridge (one call per inbound chat
//! message). Both must apply the same checks, so they live here once:
//!
//!   * the endpoint must belong to the slot we asked for,
//!   * it must be loopback on a non-zero port,
//!   * its pid must still be alive (a stale `runtime.json` outlives a
//!     crashed runtime),
//!   * and the token file must resolve inside the generated project.
//!
//! The port is ephemeral by default, so a respawned runtime comes back
//! on a *different* port. Callers must therefore re-read the endpoint
//! per request rather than caching it across the supervisor's lifetime.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::{MakakooError, Result};

/// The only name the generated runtime ever writes its token to.
const TOKEN_FILE_NAME: &str = ".runtime-token";

/// Cap on a runtime response body. The runtime is ours and answers
/// small JSON, so anything larger means something is wrong — and an
/// unbounded read on a message path is a way to be starved of memory.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// `/health` answers a three-field object.
const MAX_HEALTH_BYTES: usize = 8 * 1024;

/// The runtime writes 64 hex characters plus a newline.
const MAX_TOKEN_BYTES: usize = 4096;

/// Open a file, refusing to traverse a final-component symlink.
#[cfg(unix)]
fn open_no_symlink(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| {
            MakakooError::Config(format!(
                "open runtime token {} (refusing to follow a symlink): {e}",
                path.display()
            ))
        })
}

/// Windows has no `O_NOFOLLOW` equivalent here; the earlier canonical
/// path check is what stands.
#[cfg(not(unix))]
fn open_no_symlink(path: &Path) -> Result<std::fs::File> {
    std::fs::File::open(path)
        .map_err(|e| MakakooError::Config(format!("open runtime token {}: {e}", path.display())))
}

/// Contents of the generated project's `runtime.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeEndpoint {
    pub slot: String,
    pub engine: String,
    pub host: String,
    pub port: u16,
    pub pid: u32,
    pub token_file: PathBuf,
}

impl RuntimeEndpoint {
    /// Base URL of the loopback runtime API.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Read the bearer token the runtime generated at startup.
    ///
    /// `read_endpoint` proved the path was the right one; this proves
    /// the file still is. The two happen at different times, so the
    /// open refuses to follow a symlink rather than trusting the
    /// earlier `canonicalize` — otherwise the link could be swapped in
    /// between, and the token read from wherever it now points.
    pub fn read_token(&self) -> Result<String> {
        let file = open_no_symlink(&self.token_file)?;
        let meta = file.metadata().map_err(|e| {
            MakakooError::Config(format!(
                "stat runtime token {}: {e}",
                self.token_file.display()
            ))
        })?;
        if !meta.is_file() {
            return Err(MakakooError::Config(format!(
                "runtime token {} is not a regular file",
                self.token_file.display()
            )));
        }
        if meta.len() > MAX_TOKEN_BYTES as u64 {
            return Err(MakakooError::Config(format!(
                "runtime token {} is larger than {MAX_TOKEN_BYTES} bytes",
                self.token_file.display()
            )));
        }
        let mut raw = String::new();
        use std::io::Read;
        file.take(MAX_TOKEN_BYTES as u64)
            .read_to_string(&mut raw)
            .map_err(|e| {
                MakakooError::Config(format!(
                    "read runtime token {}: {e}",
                    self.token_file.display()
                ))
            })?;
        Ok(raw.trim().to_string())
    }
}

/// Read and validate `<project_dir>/runtime.json`.
pub fn read_endpoint(project_dir: &Path, expected_slot: &str) -> Result<RuntimeEndpoint> {
    let path = project_dir.join("runtime.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        MakakooError::Config(format!(
            "runtime metadata unavailable at {}: {} (is the slot started?)",
            path.display(),
            e
        ))
    })?;
    let info: RuntimeEndpoint = serde_json::from_str(&raw).map_err(|e| {
        MakakooError::Config(format!(
            "invalid runtime metadata {}: {}",
            path.display(),
            e
        ))
    })?;
    if info.slot != expected_slot || info.engine != "deepseek-harness" {
        return Err(MakakooError::Config(format!(
            "runtime metadata does not belong to slot '{}'",
            expected_slot
        )));
    }
    if info.host != "127.0.0.1" || info.port == 0 {
        return Err(MakakooError::Config(
            "runtime endpoint must be a non-zero loopback port".into(),
        ));
    }
    if !crate::agents::process::pid_is_alive(info.pid) {
        return Err(MakakooError::Config(format!(
            "slot '{}' is not running (stale runtime metadata for pid {})",
            expected_slot, info.pid
        )));
    }
    // Exact identity, not containment: the runtime always writes
    // `<project>/.runtime-token`, so accepting any file under the
    // project would let a symlink or a stray file stand in for it.
    let project = project_dir.canonicalize()?;
    let token = info.token_file.canonicalize()?;
    if token != project.join(TOKEN_FILE_NAME) {
        return Err(MakakooError::Config(format!(
            "runtime token file must be {}/{TOKEN_FILE_NAME}, got {}",
            project.display(),
            token.display()
        )));
    }
    Ok(info)
}

/// What a `/v1/run` call produced. A refusal is a normal outcome, not
/// an error: the caller relays the text to whoever asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The runtime answered.
    Answer(String),
    /// The runtime replied with a non-2xx status and this message.
    Refused { status: u16, message: String },
}

/// Extract the human-facing message from a non-2xx runtime body.
///
/// The runtime reports failures as `{"error": "..."}`; anything else
/// (a proxy, a truncated body) falls back to the raw text so the
/// operator still sees something actionable.
pub fn refusal_message(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body).trim().to_string();
            if text.is_empty() {
                format!("runtime returned {status} with an empty body")
            } else {
                text
            }
        })
}

/// Read a response body, refusing one larger than `limit`.
///
/// Truncating silently would be worse than refusing: a clipped body is
/// still valid-looking JSON often enough to be relayed as if it were
/// the whole answer.
async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err(MakakooError::Config(format!(
                "agent runtime response exceeded {limit} bytes"
            )));
        }
    }
    Ok(body)
}

/// Confirm the listener on the recorded port really is this slot's
/// runtime, before anything secret is sent to it.
///
/// `runtime.json` records a pid and a port, and `read_endpoint` checks
/// only that the pid is alive. A runtime killed with SIGKILL leaves the
/// file behind; if that pid is later recycled and something else binds
/// the port, a prompt would carry the user's message and the runtime
/// bearer token to an unrelated local process. `/health` is
/// unauthenticated and names the slot, which is exactly enough to
/// refuse that.
pub async fn verify_identity(
    http: &reqwest::Client,
    endpoint: &RuntimeEndpoint,
    timeout: Duration,
) -> Result<()> {
    let response = http
        .get(format!("{}/health", endpoint.base_url()))
        .timeout(timeout)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(MakakooError::Config(format!(
            "127.0.0.1:{} answered /health with {} — not this slot's runtime",
            endpoint.port,
            response.status()
        )));
    }
    // Bounded like every other body: an unproven listener must not be
    // able to make us buffer whatever it likes.
    let bytes = read_bounded(response, MAX_HEALTH_BYTES).await?;
    let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        MakakooError::Config(format!(
            "127.0.0.1:{} is not a makakoo runtime ({e})",
            endpoint.port
        ))
    })?;
    let slot = body
        .get("slot")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let engine = body
        .get("engine")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if slot != endpoint.slot || engine != "deepseek-harness" {
        return Err(MakakooError::Config(format!(
            "127.0.0.1:{} belongs to slot '{}' ({}), not '{}' — refusing to send",
            endpoint.port, slot, engine, endpoint.slot
        )));
    }
    Ok(())
}

/// How long the identity check may take. Loopback; a slow answer here
/// is itself a signal that something is wrong.
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);

/// Send one prompt to the runtime and wait for the full answer.
///
/// The endpoint's identity is confirmed first — see
/// [`verify_identity`].
pub async fn run_prompt(
    http: &reqwest::Client,
    endpoint: &RuntimeEndpoint,
    text: &str,
    session_id: &str,
    timeout: Duration,
) -> Result<RunOutcome> {
    verify_identity(http, endpoint, IDENTITY_TIMEOUT).await?;
    let token = endpoint.read_token()?;
    let response = http
        .post(format!("{}/v1/run", endpoint.base_url()))
        .bearer_auth(token)
        .timeout(timeout)
        .json(&serde_json::json!({"text": text, "session_id": session_id}))
        .send()
        .await?;
    let status = response.status();
    let bytes = read_bounded(response, MAX_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Ok(RunOutcome::Refused {
            status: status.as_u16(),
            message: refusal_message(status.as_u16(), &bytes),
        });
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        MakakooError::Config(format!(
            "agent runtime returned {} with invalid JSON: {} ({})",
            status,
            e,
            String::from_utf8_lossy(&bytes)
        ))
    })?;
    let answer = body
        .get("response")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MakakooError::Config("agent runtime response missing 'response'".into()))?;
    Ok(RunOutcome::Answer(answer.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_runtime(dir: &Path, body: serde_json::Value) {
        std::fs::write(dir.join("runtime.json"), body.to_string()).unwrap();
    }

    fn with_token(dir: &Path) -> PathBuf {
        let token = dir.join(".runtime-token");
        std::fs::write(&token, "secret").unwrap();
        token
    }

    #[test]
    fn endpoint_rejects_remote_host() {
        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "0.0.0.0", "port": 9000,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(error.to_string().contains("loopback"), "{error}");
    }

    #[test]
    fn endpoint_accepts_scoped_token_file() {
        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let endpoint = read_endpoint(tmp.path(), "researcher").unwrap();
        assert_eq!(endpoint.port, 9000);
        assert_eq!(endpoint.base_url(), "http://127.0.0.1:9000");
        assert_eq!(endpoint.read_token().unwrap(), "secret");
    }

    #[test]
    fn endpoint_rejects_stale_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": 0, "token_file": token,
            }),
        );
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(
            error.to_string().contains("stale runtime metadata"),
            "{error}"
        );
    }

    #[test]
    fn endpoint_rejects_another_slots_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "someone-else", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(error.to_string().contains("does not belong"), "{error}");
    }

    #[tokio::test]
    async fn a_recycled_port_is_refused_before_anything_secret_is_sent() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Something else is listening where runtime.json says our slot
        // should be. It must never receive the prompt or the token.
        let impostor = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": true, "slot": "someone-else", "engine": "deepseek-harness"}),
            ))
            .mount(&impostor)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"session_id": "x", "response": "should never happen"}),
            ))
            .mount(&impostor)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        let port: u16 = impostor.uri().rsplit(':').next().unwrap().parse().unwrap();
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": port,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let endpoint = read_endpoint(tmp.path(), "researcher").unwrap();
        let http = reqwest::Client::new();
        let error = run_prompt(
            &http,
            &endpoint,
            "my private question",
            "tg:1",
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("someone-else"), "{error}");

        let seen = impostor.received_requests().await.unwrap();
        assert!(
            seen.iter().all(|r| r.url.path() == "/health"),
            "nothing but the identity probe may reach a foreign listener"
        );
    }

    #[tokio::test]
    async fn a_matching_runtime_is_accepted() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let runtime = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": true, "slot": "researcher", "engine": "deepseek-harness"}),
            ))
            .mount(&runtime)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"session_id": "s", "response": "hello"})),
            )
            .mount(&runtime)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        let port: u16 = runtime.uri().rsplit(':').next().unwrap().parse().unwrap();
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": port,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let endpoint = read_endpoint(tmp.path(), "researcher").unwrap();
        let outcome = run_prompt(
            &reqwest::Client::new(),
            &endpoint,
            "hi",
            "tg:1",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(outcome, RunOutcome::Answer("hello".into()));
    }

    #[test]
    fn a_token_file_that_is_not_the_runtime_token_is_refused() {
        // Containment is not identity: a decoy inside the project must
        // not stand in for the file the runtime actually writes.
        let tmp = tempfile::tempdir().unwrap();
        with_token(tmp.path());
        let decoy = tmp.path().join("decoy");
        std::fs::write(&decoy, "attacker-controlled").unwrap();
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": std::process::id(), "token_file": decoy,
            }),
        );
        let error = read_endpoint(tmp.path(), "researcher").unwrap_err();
        assert!(error.to_string().contains(".runtime-token"), "{error}");
    }

    #[test]
    fn a_symlinked_token_file_pointing_outside_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("elsewhere");
        std::fs::write(&secret, "not ours").unwrap();
        let link = tmp.path().join(".runtime-token");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "not ours").unwrap();
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": std::process::id(), "token_file": link,
            }),
        );
        let result = read_endpoint(tmp.path(), "researcher");
        #[cfg(unix)]
        assert!(
            result.is_err(),
            "a token symlinked out of the project must be refused"
        );
        #[cfg(not(unix))]
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_rather_than_silently_truncated() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let runtime = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": true, "slot": "researcher", "engine": "deepseek-harness"}),
            ))
            .mount(&runtime)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/run"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "session_id": "s", "response": "x".repeat(2 * 1024 * 1024)
            })))
            .mount(&runtime)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let token = with_token(tmp.path());
        let port: u16 = runtime.uri().rsplit(':').next().unwrap().parse().unwrap();
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": port,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let endpoint = read_endpoint(tmp.path(), "researcher").unwrap();
        let error = run_prompt(
            &reqwest::Client::new(),
            &endpoint,
            "hi",
            "tg:1",
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeded"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_token_symlink_swapped_in_after_validation_is_refused() {
        // The endpoint validates a real file, then the link is put in
        // its place before the token is read — the check/use gap.
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("stolen"), "someone-elses-token").unwrap();
        let token = with_token(tmp.path());
        write_runtime(
            tmp.path(),
            serde_json::json!({
                "slot": "researcher", "engine": "deepseek-harness",
                "host": "127.0.0.1", "port": 9000,
                "pid": std::process::id(), "token_file": token,
            }),
        );
        let endpoint = read_endpoint(tmp.path(), "researcher").unwrap();
        assert_eq!(endpoint.read_token().unwrap(), "secret");

        std::fs::remove_file(tmp.path().join(".runtime-token")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("stolen"),
            tmp.path().join(".runtime-token"),
        )
        .unwrap();
        let error = endpoint.read_token().unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn refusal_message_prefers_the_runtime_error_field() {
        let body = br#"{"error":"upstream 502","code":"provider_error"}"#;
        assert_eq!(refusal_message(502, body), "upstream 502");
    }

    #[test]
    fn refusal_message_falls_back_to_raw_body() {
        assert_eq!(refusal_message(500, b"  boom  "), "boom");
        assert!(refusal_message(500, b"").contains("empty body"));
    }
}
