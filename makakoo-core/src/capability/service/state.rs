//! `StateHandler` — read/write/list under `$MAKAKOO_HOME/state/<plugin>/`.
//!
//! Spec §1.4: every plugin with a `[state]` table in its manifest gets
//! a `state/plugin` grant scoped to its own state dir. This handler
//! serves those calls over the capability socket.
//!
//! **Path jailing:** incoming paths are treated as relative to the
//! plugin's state dir. Absolute paths and any component resolving to
//! `..` are rejected. A plugin with `state/plugin` grant on
//! `$MAKAKOO_HOME/state/arbitrage` cannot read `/etc/passwd` or
//! `../watchdog-postgres/secret.txt` — the `state/plugin` grant is not
//! a filesystem escape hatch.
//!
//! **Methods served:**
//! - `state.read` params `{ path }` → `{ bytes_b64 }`
//! - `state.write` params `{ path, bytes_b64 }` → `{ bytes_written }`
//! - `state.list` params `{ path? }` → `{ entries: [{ name, is_dir }] }`
//! - `state.delete` params `{ path }` → `{ removed }`
//!
//! Bytes are base64-encoded in the JSON envelope. Most state is text
//! (JSON, TOML, JSONL) so the b64 overhead is acceptable; the
//! alternative (out-of-band binary framing) isn't worth the complexity
//! at this phase.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::capability::socket::{
    CapabilityError, CapabilityHandler, CapabilityRequest,
};

#[derive(Debug, Error)]
pub enum StateError {
    #[error("absolute paths are not allowed in state operations")]
    Absolute,
    #[error("parent traversal (..) is not allowed in state operations")]
    ParentTraversal,
    #[error("empty path")]
    EmptyPath,
}

/// Read/write to a plugin's state dir. The dir is created on first
/// write if it doesn't exist.
pub struct StateHandler {
    root: PathBuf,
}

impl StateHandler {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a caller-supplied relative path against the root, rejecting
    /// absolute paths and any parent traversal.
    fn jail(&self, rel: &str) -> Result<PathBuf, StateError> {
        if rel.is_empty() {
            return Err(StateError::EmptyPath);
        }
        let rel_path = Path::new(rel);
        if rel_path.is_absolute() {
            return Err(StateError::Absolute);
        }
        for c in rel_path.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(StateError::ParentTraversal);
                }
            }
        }
        Ok(self.root.join(rel_path))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ReadParams {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct WriteParams {
    path: String,
    bytes_b64: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct ListParams {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct DeleteParams {
    path: String,
}

#[derive(Debug, Serialize)]
struct Entry {
    name: String,
    is_dir: bool,
}

#[async_trait]
impl CapabilityHandler for StateHandler {
    async fn handle(
        &self,
        request: &CapabilityRequest,
        _scope: Option<&str>,
    ) -> Result<serde_json::Value, CapabilityError> {
        match request.method.as_str() {
            "state.read" => {
                let p: ReadParams = parse_params(&request.params)?;
                let target = self
                    .jail(&p.path)
                    .map_err(|e| CapabilityError::bad_request(e.to_string()))?;
                let bytes = tokio::fs::read(&target).await.map_err(|e| {
                    CapabilityError::handler(format!(
                        "state.read {:?}: {e}",
                        target.display()
                    ))
                })?;
                Ok(serde_json::json!({
                    "bytes_b64": BASE64.encode(&bytes),
                    "len": bytes.len(),
                }))
            }
            "state.write" => {
                let p: WriteParams = parse_params(&request.params)?;
                let target = self
                    .jail(&p.path)
                    .map_err(|e| CapabilityError::bad_request(e.to_string()))?;
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        CapabilityError::handler(format!(
                            "mkdir {:?}: {e}",
                            parent.display()
                        ))
                    })?;
                }
                let bytes = BASE64
                    .decode(p.bytes_b64.as_bytes())
                    .map_err(|e| CapabilityError::bad_request(format!("bad b64: {e}")))?;
                let n = bytes.len();
                tokio::fs::write(&target, &bytes).await.map_err(|e| {
                    CapabilityError::handler(format!(
                        "state.write {:?}: {e}",
                        target.display()
                    ))
                })?;
                Ok(serde_json::json!({ "bytes_written": n }))
            }
            "state.list" => {
                let p: ListParams = parse_params(&request.params).unwrap_or_default();
                let dir = match p.path.as_deref() {
                    Some(sub) if !sub.is_empty() => self
                        .jail(sub)
                        .map_err(|e| CapabilityError::bad_request(e.to_string()))?,
                    _ => self.root.clone(),
                };
                if !dir.exists() {
                    return Ok(serde_json::json!({ "entries": Vec::<Entry>::new() }));
                }
                let mut out: Vec<Entry> = Vec::new();
                let mut rd = tokio::fs::read_dir(&dir).await.map_err(|e| {
                    CapabilityError::handler(format!(
                        "list {:?}: {e}",
                        dir.display()
                    ))
                })?;
                while let Some(e) = rd.next_entry().await.map_err(|e| {
                    CapabilityError::handler(format!("list iter: {e}"))
                })? {
                    let is_dir = e
                        .file_type()
                        .await
                        .map(|t| t.is_dir())
                        .unwrap_or(false);
                    out.push(Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        is_dir,
                    });
                }
                out.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(serde_json::to_value(&serde_json::json!({ "entries": out }))
                    .unwrap())
            }
            "state.delete" => {
                let p: DeleteParams = parse_params(&request.params)?;
                let target = self
                    .jail(&p.path)
                    .map_err(|e| CapabilityError::bad_request(e.to_string()))?;
                if !target.exists() {
                    return Ok(serde_json::json!({ "removed": false }));
                }
                let md = tokio::fs::metadata(&target).await.map_err(|e| {
                    CapabilityError::handler(format!("stat: {e}"))
                })?;
                if md.is_dir() {
                    tokio::fs::remove_dir_all(&target).await.map_err(|e| {
                        CapabilityError::handler(format!("rmdir: {e}"))
                    })?;
                } else {
                    tokio::fs::remove_file(&target).await.map_err(|e| {
                        CapabilityError::handler(format!("unlink: {e}"))
                    })?;
                }
                Ok(serde_json::json!({ "removed": true }))
            }
            other => Err(CapabilityError::handler(format!(
                "unknown state method {other:?}"
            ))),
        }
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(
    v: &serde_json::Value,
) -> Result<T, CapabilityError> {
    serde_json::from_value(v.clone())
        .map_err(|e| CapabilityError::bad_request(format!("bad params: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn req(method: &str, params: serde_json::Value) -> CapabilityRequest {
        CapabilityRequest {
            id: json!(1),
            method: method.to_string(),
            params,
            verb: "state/plugin".into(),
            scope: String::new(),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());

        let bytes = b"hello plugin state";
        let write_params = json!({
            "path": "foo/bar.json",
            "bytes_b64": BASE64.encode(bytes),
        });
        let w = h.handle(&req("state.write", write_params), None).await.unwrap();
        assert_eq!(w["bytes_written"], bytes.len());

        let r = h
            .handle(&req("state.read", json!({ "path": "foo/bar.json" })), None)
            .await
            .unwrap();
        let decoded = BASE64.decode(r["bytes_b64"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, bytes);
        assert_eq!(r["len"], bytes.len());
    }

    #[tokio::test]
    async fn absolute_path_rejected() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());
        let err = h
            .handle(&req("state.read", json!({ "path": "/etc/passwd" })), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("absolute"));
    }

    #[tokio::test]
    async fn parent_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());
        let err = h
            .handle(
                &req("state.read", json!({ "path": "../evil" })),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("parent"));
    }

    #[tokio::test]
    async fn list_returns_sorted_entries() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());
        for name in ["zeta.txt", "alpha.txt", "beta.txt"] {
            let p = json!({
                "path": name,
                "bytes_b64": BASE64.encode(b"x"),
            });
            h.handle(&req("state.write", p), None).await.unwrap();
        }
        let r = h.handle(&req("state.list", json!({})), None).await.unwrap();
        let entries = r["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha.txt", "beta.txt", "zeta.txt"]);
    }

    #[tokio::test]
    async fn delete_removes_file_and_reports_removed_false_when_missing() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());
        let p = json!({ "path": "x.txt", "bytes_b64": BASE64.encode(b"x") });
        h.handle(&req("state.write", p), None).await.unwrap();

        let r = h
            .handle(&req("state.delete", json!({ "path": "x.txt" })), None)
            .await
            .unwrap();
        assert_eq!(r["removed"], true);

        let r = h
            .handle(&req("state.delete", json!({ "path": "x.txt" })), None)
            .await
            .unwrap();
        assert_eq!(r["removed"], false);
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let tmp = TempDir::new().unwrap();
        let h = StateHandler::new(tmp.path().to_path_buf());
        let err = h
            .handle(&req("state.teleport", json!({})), None)
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown state method"));
    }

    #[tokio::test]
    async fn list_empty_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let empty = tmp.path().join("empty");
        let h = StateHandler::new(empty);
        let r = h.handle(&req("state.list", json!({})), None).await.unwrap();
        assert!(r["entries"].as_array().unwrap().is_empty());
    }
}
