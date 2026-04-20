//! Tier-B `harvey_knowledge_ingest` handler — structured media ingestion.
//!
//! Sebastian's canonical fix for the URL-in-journal confabulation bug
//! (opencode, 2026-04-20): when the user asks to *add / save / index* a
//! video / audio / pdf / image / article, the model must NOT paper over a
//! rate-limited `describe_*` by journaling the URL. It must call
//! `harvey_knowledge_ingest`, which shells out to the Python
//! `multimodal-knowledge` agent and persists real content into the
//! "multimodal" Qdrant collection.
//!
//! The handler is intentionally a thin, safety-gated dispatcher — all the
//! embedding + chunking logic lives in Python where the Gemini + Qdrant
//! clients already exist. The Rust side only validates arguments, spawns
//! the subprocess, and parses the `--json` line it emits on stdout.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::timeout;

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

/// Tool timeout. Video downloads + embedding can be slow; 20 min is the
/// budget. Anything longer is almost certainly a hang.
const INGEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Valid `kind` values — must match ingest.py's argparse choices.
const VALID_KINDS: &[&str] = &["video", "audio", "pdf", "image", "text"];

/// Resolve `$MAKAKOO_HOME` with `~/MAKAKOO` as fallback.
fn resolve_makakoo_home() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("MAKAKOO_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("HARVEY_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    dirs::home_dir().map(|h| h.join("MAKAKOO"))
}

/// Pick a Python interpreter that has the multimodal deps installed.
/// Honours `$MAKAKOO_PYTHON`, falls back to `python3.11`, then `python3`.
/// Validation of the actual dependency set (qdrant_client, google.genai)
/// happens when the script imports them — this resolver only decides
/// which binary to invoke.
fn resolve_python() -> String {
    if let Ok(v) = std::env::var("MAKAKOO_PYTHON") {
        if !v.is_empty() {
            return v;
        }
    }
    // Prefer 3.11 because Sebastian's macOS baseline has qdrant-client
    // installed there, not on the Xcode-shipped python3 (3.9).
    if std::process::Command::new("python3.11")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "python3.11".to_string();
    }
    "python3".to_string()
}

/// Resolve `ingest.py` path. Requires both $MAKAKOO_HOME and the file to
/// exist; otherwise returns a user-facing error.
fn resolve_ingest_script() -> Result<PathBuf, String> {
    let home = resolve_makakoo_home()
        .ok_or_else(|| "cannot resolve $MAKAKOO_HOME (and $HOME is unset)".to_string())?;
    let script = home.join("agents/multimodal-knowledge/ingest.py");
    if !script.exists() {
        return Err(format!(
            "ingest.py not found at {} — did you forget to clone the \
             multimodal-knowledge agent?",
            script.display()
        ));
    }
    Ok(script)
}

/// Normalise and validate the caller-supplied `kind` argument. Returns
/// `Ok(None)` if unset (ingest.py auto-detects from extension); returns
/// `Err` with a user-facing message on unknown values.
fn validate_kind(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(k) => {
            let normalised = k.trim().to_lowercase();
            if normalised.is_empty() {
                return Ok(None);
            }
            if !VALID_KINDS.contains(&normalised.as_str()) {
                return Err(format!(
                    "invalid `kind`: {k:?} — must be one of {VALID_KINDS:?}"
                ));
            }
            Ok(Some(normalised))
        }
    }
}

/// Normalise and validate the `source` argument. A non-empty string is
/// required; we do NOT pre-validate URL vs path — ingest.py resolves both.
fn validate_source(raw: Option<&str>) -> Result<String, String> {
    match raw {
        None => Err("missing required `source` (URL or absolute path)".to_string()),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err("`source` must be a non-empty URL or path".to_string());
            }
            Ok(trimmed.to_string())
        }
    }
}

pub struct HarveyKnowledgeIngestHandler {
    ctx: Arc<ToolContext>,
}

impl HarveyKnowledgeIngestHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for HarveyKnowledgeIngestHandler {
    fn name(&self) -> &str {
        "harvey_knowledge_ingest"
    }

    fn description(&self) -> &str {
        "Add media to Harvey's multimodal knowledge store (Qdrant + Gemini \
         Embedding 2). `source` is a URL or absolute path to a video, \
         audio file, PDF, image, or text. Use this when the user says \
         add / save / remember / index / ingest / store — NOT `harvey_describe_*`, \
         which is one-shot Q&A with no persistence. Returns doc_ids that \
         future superbrain queries will retrieve by content."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "URL (http/https; YouTube routed through yt-dlp) or absolute local path."
                },
                "kind": {
                    "type": "string",
                    "enum": VALID_KINDS,
                    "description": "Override auto-detected file type. Omit to auto-detect from extension."
                },
                "title": {
                    "type": "string",
                    "description": "Display title for the Qdrant record (defaults to filename)."
                },
                "note": {
                    "type": "string",
                    "description": "Free-form metadata note persisted alongside each chunk."
                }
            },
            "required": ["source"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let source = validate_source(params.get("source").and_then(|v| v.as_str()))
            .map_err(|e| RpcError::invalid_params(&e))?;
        let kind = validate_kind(params.get("kind").and_then(|v| v.as_str()))
            .map_err(|e| RpcError::invalid_params(&e))?;
        let title = params.get("title").and_then(|v| v.as_str());
        let note = params.get("note").and_then(|v| v.as_str());

        let script = resolve_ingest_script()
            .map_err(|e| RpcError::internal(&e))?;
        let python = resolve_python();

        let mut cmd = Command::new(&python);
        cmd.arg(&script)
            .arg("--source").arg(&source)
            .arg("--json");
        if let Some(k) = &kind {
            cmd.arg("--kind").arg(k);
        }
        if let Some(t) = title {
            cmd.arg("--title").arg(t);
        }
        if let Some(n) = note {
            cmd.arg("--note").arg(n);
        }

        // Preserve $MAKAKOO_HOME for ingest.py's downstream calls to
        // video_ingest.py (which reads $HARVEY_HOME/data/video-ocr/).
        if let Ok(home) = std::env::var("MAKAKOO_HOME") {
            cmd.env("MAKAKOO_HOME", home.clone());
            cmd.env("HARVEY_HOME", home);
        }

        let output = timeout(INGEST_TIMEOUT, cmd.output())
            .await
            .map_err(|_| RpcError::internal(&format!(
                "ingest timed out after {}s",
                INGEST_TIMEOUT.as_secs()
            )))?
            .map_err(|e| RpcError::internal(&format!("failed to spawn {python}: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Satisfy "fields get touched" lint on future ToolContext expansion.
        let _ = self.ctx.home.clone();

        // ingest.py emits exactly one JSON line on stdout. Take the last
        // non-empty line (stderr warnings sometimes bleed into stdout
        // depending on Python deprecation noise routing).
        let json_line = stdout
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with('{') && t.ends_with('}')
            })
            .last()
            .unwrap_or("");

        let parsed: Value = serde_json::from_str(json_line).unwrap_or_else(|_| json!({
            "ingested": false,
            "doc_ids": [],
            "errors": [format!("could not parse ingest.py stdout as JSON (exit {exit_code})")],
            "summary": stderr.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | "),
        }));

        Ok(json!({
            "ok": exit_code == 0 && parsed.get("ingested").and_then(Value::as_bool).unwrap_or(false),
            "exit_code": exit_code,
            "ingested": parsed.get("ingested").cloned().unwrap_or(Value::Bool(false)),
            "doc_ids": parsed.get("doc_ids").cloned().unwrap_or(json!([])),
            "chunks": parsed.get("chunks").cloned().unwrap_or(json!(0)),
            "file_type": parsed.get("file_type").cloned().unwrap_or(Value::Null),
            "summary": parsed.get("summary").cloned().unwrap_or(Value::Null),
            "errors": parsed.get("errors").cloned().unwrap_or(json!([])),
            "source": source,
            "resolved_path": parsed.get("resolved_path").cloned().unwrap_or(Value::Null),
            "stderr_tail": stderr.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_handler() -> HarveyKnowledgeIngestHandler {
        let tmp = TempDir::new().unwrap();
        let ctx = Arc::new(ToolContext::empty(tmp.path().to_path_buf()));
        HarveyKnowledgeIngestHandler::new(ctx)
    }

    #[test]
    fn handler_metadata() {
        let h = make_handler();
        assert_eq!(h.name(), "harvey_knowledge_ingest");
        let desc = h.description();
        assert!(desc.to_lowercase().contains("ingest"));
        assert!(desc.to_lowercase().contains("describe_"));
        let schema = h.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "source");
        // enum must contain the five kinds ingest.py accepts.
        let enum_vals = schema["properties"]["kind"]["enum"]
            .as_array()
            .expect("kind enum missing");
        let set: std::collections::HashSet<_> =
            enum_vals.iter().filter_map(|v| v.as_str()).collect();
        for k in VALID_KINDS {
            assert!(set.contains(k), "schema missing kind {k}");
        }
    }

    // --- argument validation --------------------------------------------

    #[test]
    fn validate_source_rejects_missing() {
        let err = validate_source(None).unwrap_err();
        assert!(err.contains("missing required"));
    }

    #[test]
    fn validate_source_rejects_empty() {
        let err = validate_source(Some("   ")).unwrap_err();
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn validate_source_accepts_url() {
        let s = validate_source(Some("https://youtu.be/abc ")).unwrap();
        assert_eq!(s, "https://youtu.be/abc");
    }

    #[test]
    fn validate_source_accepts_path() {
        let s = validate_source(Some("/tmp/foo.mp4")).unwrap();
        assert_eq!(s, "/tmp/foo.mp4");
    }

    #[test]
    fn validate_kind_allows_none() {
        assert_eq!(validate_kind(None).unwrap(), None);
        assert_eq!(validate_kind(Some("")).unwrap(), None);
        assert_eq!(validate_kind(Some("   ")).unwrap(), None);
    }

    #[test]
    fn validate_kind_accepts_known() {
        for k in VALID_KINDS {
            assert_eq!(validate_kind(Some(k)).unwrap().as_deref(), Some(*k));
        }
        // Case-insensitive normalisation.
        assert_eq!(
            validate_kind(Some("VIDEO")).unwrap().as_deref(),
            Some("video")
        );
    }

    #[test]
    fn validate_kind_rejects_unknown() {
        let err = validate_kind(Some("zipfile")).unwrap_err();
        assert!(err.contains("invalid `kind`"));
        assert!(err.contains("zipfile"));
    }

    // --- script resolution ----------------------------------------------

    #[test]
    fn resolve_ingest_script_errors_when_absent() {
        // Scope MAKAKOO_HOME to a tempdir that definitely has no agents/.
        let tmp = TempDir::new().unwrap();
        let old = std::env::var("MAKAKOO_HOME").ok();
        std::env::set_var("MAKAKOO_HOME", tmp.path());
        let err = resolve_ingest_script().unwrap_err();
        assert!(err.contains("ingest.py not found"));
        match old {
            Some(v) => std::env::set_var("MAKAKOO_HOME", v),
            None => std::env::remove_var("MAKAKOO_HOME"),
        }
    }

    #[test]
    fn resolve_ingest_script_finds_when_present() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("agents/multimodal-knowledge");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("ingest.py"), "# stub").unwrap();
        let old = std::env::var("MAKAKOO_HOME").ok();
        std::env::set_var("MAKAKOO_HOME", tmp.path());
        let found = resolve_ingest_script().unwrap();
        assert!(found.ends_with("agents/multimodal-knowledge/ingest.py"));
        match old {
            Some(v) => std::env::set_var("MAKAKOO_HOME", v),
            None => std::env::remove_var("MAKAKOO_HOME"),
        }
    }

    // --- call-path error shaping (no python subprocess spawned) ---------

    #[tokio::test]
    async fn call_missing_source_returns_invalid_params() {
        let h = make_handler();
        let err = h.call(json!({})).await.unwrap_err();
        assert!(err.message.contains("missing required"));
    }

    #[tokio::test]
    async fn call_empty_source_returns_invalid_params() {
        let h = make_handler();
        let err = h.call(json!({"source": ""})).await.unwrap_err();
        assert!(err.message.contains("non-empty"));
    }

    #[tokio::test]
    async fn call_bad_kind_returns_invalid_params() {
        let h = make_handler();
        let err = h
            .call(json!({"source": "/tmp/x.mp4", "kind": "zipfile"}))
            .await
            .unwrap_err();
        assert!(err.message.contains("invalid `kind`"));
    }
}
