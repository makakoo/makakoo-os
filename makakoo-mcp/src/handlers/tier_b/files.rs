//! Tier-B `write_file` — the sandboxed file write behind `spec/USER_GRANTS.md`.
//!
//! This is a **port** of the Python implementation in
//! `plugins-core/lib-harvey-core/src/core/agent/harvey_agent.py`
//! (`_resolve_write_path`, `_write_file_rejection_message`,
//! `tool_write_file`), not a new design. The three-layer model is
//! `spec/CAPABILITIES.md §1.11`; the conversational UX this rejection text
//! feeds is `spec/USER_GRANTS.md §12` Flow 1.
//!
//! ## Two callers, two authorisation models
//!
//! | caller | authority | on refusal |
//! |---|---|---|
//! | agent slot (`MAKAKOO_AGENT_SLOT` set) | the slot's `allowed_paths` / `forbidden_paths` | hard error — a scoped agent writing outside its scope is a violation, not a negotiation |
//! | no slot (Harvey / operator) | baseline roots + active user grants | a *conversation*: the result carries the exact `grant_write_access` call that would unblock it, per §12 Flow 1 |
//!
//! The slot path is already gated upstream: `server.rs::authorize_tool` runs
//! `check_path` over every filesystem-shaped argument before dispatch, which
//! is why the parameter **must** stay named `path`. The check here is
//! deliberate defence in depth — it re-runs against the *resolved* path, so a
//! symlink that the raw-argument check could not see is still caught.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};

use makakoo_core::capability::{AuditEntry, AuditLog, AuditResult, UserGrants};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use super::perms::{baseline_roots_for, glob_form};

/// Which layer authorised a write. Carried into the audit line so a later
/// review can tell a baseline write from a granted one.
enum Authority {
    /// The slot's own `allowed_paths`. `String` is the slot id.
    Slot(String),
    /// A user grant explicitly bound to this slot. `String` is the grant id.
    SlotGrant(String),
    Baseline(PathBuf),
    Grant(String),
}

impl Authority {
    fn audit_scope(&self) -> String {
        match self {
            Authority::Slot(slot_id) => format!("slot:{slot_id}"),
            Authority::SlotGrant(id) | Authority::Grant(id) => id.clone(),
            Authority::Baseline(root) => format!("baseline:{}", root.display()),
        }
    }
}

pub struct WriteFileHandler {
    ctx: Arc<ToolContext>,
}

impl WriteFileHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }

    /// Layer 1 + Layer 3 for a caller with no agent slot.
    fn authorize_unscoped(&self, resolved: &Path) -> Result<Authority, String> {
        for root in baseline_roots_for(&self.ctx.home) {
            let Some(root_canon) = makakoo_core::agents::resolve_scope_path(Path::new(&root))
            else {
                continue;
            };
            // Prefix comparison on canonical paths is the Rust equivalent of
            // Python's commonpath check: `/tmp/foo-evil` must not match a
            // `/tmp/foo` root, so compare component-wise, not as strings.
            if resolved.starts_with(&root_canon) {
                return Ok(Authority::Baseline(root_canon));
            }
        }
        let Some(resolved_str) = utf8_path(resolved) else {
            return Err(format!(
                "write_file rejected: '{}' is not valid UTF-8; grant scopes are \
                 UTF-8 globs and cannot be matched against it",
                resolved.display()
            ));
        };
        match self.matching_grant(resolved_str, None) {
            Some(id) => Ok(Authority::Grant(id)),
            None => Err(self.rejection_message(resolved_str)),
        }
    }

    /// First active grant covering `resolved`, restricted to the grants the
    /// caller may use.
    ///
    /// `bound_to` distinguishes the two callers, and the difference is not
    /// `visible_to`: that returns `true` for machine-global grants against
    /// *any* caller, which would let an operator granting themselves a path
    /// silently widen every running agent. A slot may use only grants bound
    /// to it by name.
    fn matching_grant(&self, resolved: &str, bound_to: Option<&str>) -> Option<String> {
        UserGrants::load(&self.ctx.home)
            .active_grants(Utc::now())
            .into_iter()
            .filter(|grant| match bound_to {
                Some(slot_id) => grant.bound_to_agent.as_deref() == Some(slot_id),
                // Operator dispatch: machine-global grants only. A grant
                // bound to some agent is that agent's, not the operator's.
                None => grant.bound_to_agent.is_none(),
            })
            // Compared in glob form: the stored scope is a glob with `/`
            // separators, so a Windows path must be presented the same
            // way or it can never match the grant made for it.
            .find(|grant| grant.matches_path(&glob_form(resolved)))
            .map(|grant| grant.id.clone())
    }

    /// The rejection the LLM reads. Its whole job is to be quotable: it names
    /// what is allowed today and the exact call that would unblock the write,
    /// so the assistant can ask for consent instead of guessing.
    fn rejection_message(&self, requested: &str) -> String {
        let baseline = baseline_roots_for(&self.ctx.home).join(", ");
        let grants = UserGrants::load(&self.ctx.home);
        let active: Vec<String> = grants
            .active_grants(Utc::now())
            .into_iter()
            .filter(|grant| grant.bound_to_agent.is_none())
            .filter(|grant| grant.scope.starts_with("fs/write:"))
            .map(|grant| format!("    • {} (grant {})", grant.scope, grant.id))
            .collect();
        let grants_block = if active.is_empty() {
            "  (none)".to_string()
        } else {
            active.join("\n")
        };
        let suggested = suggest_grant_scope(requested);
        format!(
            "write_file rejected: '{requested}' is outside the allowed baseline roots and active grants.\n\
             \x20 Baseline: {baseline}\n\
             \x20 Active grants:\n{grants_block}\n\
             Ask the user for consent, then unblock with: grant_write_access(path=\"{suggested}\", duration=\"1h\")\n\
             CLI equivalent: makakoo perms grant '{suggested}' --for 1h"
        )
    }

    fn emit_audit(
        &self,
        requested: &str,
        outcome: AuditResult,
        authority: Option<&Authority>,
        bytes: Option<u64>,
    ) {
        let Ok(log) = AuditLog::open_default(&self.ctx.home) else {
            // Nothing to fall back to — but never let an audit failure
            // silently become a *successful* unaudited write, so say so.
            eprintln!("write_file: audit log unavailable for {requested}");
            return;
        };
        let entry = AuditEntry {
            ts: Utc::now(),
            plugin: std::env::var("HARVEY_PLUGIN").unwrap_or_else(|_| "mcp".to_string()),
            plugin_version: env!("CARGO_PKG_VERSION").to_string(),
            verb: "fs/write".to_string(),
            scope_requested: requested.to_string(),
            scope_granted: authority.map(Authority::audit_scope),
            result: outcome,
            duration_ms: None,
            bytes_in: bytes,
            bytes_out: None,
            correlation_id: None,
        };
        if log.append(&entry).is_err() {
            eprintln!("write_file: audit append failed for {requested}");
        }
    }
}

/// Is `resolved` under one of the slot's `forbidden_paths`?
///
/// `ScopeError` collapses "outside allowed_paths" and "explicitly forbidden"
/// into one variant, so the distinction has to be recomputed here. It matters:
/// `forbidden_paths` is the one boundary a runtime grant must never cross.
/// The spec's layers are additive, but an explicit denial is a statement
/// about *this slot* that a grant issued elsewhere should not overturn.
///
/// Prefix handling mirrors `scope.rs::canonicalise_scope_prefix` so the two
/// cannot disagree about what a `/**` scope covers.
fn is_forbidden_for(slot: &makakoo_core::agents::AgentSlot, resolved: &Path) -> bool {
    slot.forbidden_paths
        .iter()
        .filter_map(|configured| {
            let prefix = configured
                .strip_suffix("/**")
                .or_else(|| configured.strip_suffix("/*"))
                .unwrap_or(configured);
            makakoo_core::agents::resolve_scope_path(Path::new(prefix))
        })
        .any(|forbidden| resolved.starts_with(&forbidden))
}

/// Grant scopes are UTF-8 globs. A resolved path that is not valid UTF-8
/// cannot be compared against one, and `to_string_lossy` would map distinct
/// byte sequences onto the same `U+FFFD` string — turning "no match" into a
/// possible false match against a grant for a different file.
fn utf8_path(path: &Path) -> Option<&str> {
    path.to_str()
}

/// Suggest the directory a grant should cover: the target's parent, so one
/// grant covers the sibling files the same task will touch next.
fn suggest_grant_scope(requested: &str) -> String {
    let path = Path::new(requested);
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    match parent {
        // The suggestion is meant to be pasted straight into
        // grant_write_access, which stores it as a glob.
        Some(dir) => format!("{}/", glob_form(&dir.display().to_string())),
        None => requested.to_string(),
    }
}

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write UTF-8 text to a file inside the write sandbox. Allowed targets \
         are the baseline roots ($MAKAKOO_HOME/data/reports, \
         $MAKAKOO_HOME/data/drafts, $MAKAKOO_HOME/tmp, /tmp) plus any path \
         covered by an active user grant; an agent slot is instead limited to \
         its own allowed_paths. Writes are atomic and never follow a symlink. \
         A rejection tells you the exact grant_write_access call that would \
         unblock it — ask the user before making that call."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                // MUST stay named `path`: server.rs::authorize_tool only
                // scope-checks arguments whose key is filesystem-shaped.
                "path": {
                    "type": "string",
                    "description": "Absolute path, or ~/… , or relative to $MAKAKOO_HOME"
                },
                "content": {
                    "type": "string",
                    "description": "UTF-8 text to write. Max 1 MiB."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let raw_path = params
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| RpcError::invalid_params("missing 'path'"))?;
        let content = params
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("missing 'content'"))?;

        if content.len() > makakoo_core::agents::MAX_WRITE_BYTES {
            return Err(RpcError::invalid_params(format!(
                "content is {} bytes; write_file accepts at most {}",
                content.len(),
                makakoo_core::agents::MAX_WRITE_BYTES
            )));
        }

        // Root a relative path exactly as the upstream gate does, so the path
        // this handler authorises is the path the gate already checked.
        let candidate = {
            let path = PathBuf::from(raw_path);
            if path.is_absolute() || raw_path.starts_with("~/") {
                path
            } else {
                self.ctx.home.join(path)
            }
        };
        let Some(resolved) = makakoo_core::agents::resolve_scope_path(&candidate) else {
            // A dangling symlink cannot be proven to stay in scope.
            self.emit_audit(raw_path, AuditResult::Denied, None, None);
            return Err(RpcError::invalid_params(format!(
                "write_file rejected: '{raw_path}' resolves through a dangling symlink"
            )));
        };

        let authority = match crate::dispatch::current_agent_id() {
            Some(slot_id) => {
                let slot_path = makakoo_core::agents::checked_slot_path(&self.ctx.home, &slot_id)
                    .map_err(|e| {
                    RpcError::internal(format!("invalid agent slot '{slot_id}': {e}"))
                })?;
                let slot = makakoo_core::agents::AgentSlot::load_from_file(&slot_path)
                    .map_err(|e| RpcError::internal(format!("agent slot '{slot_id}': {e}")))?;
                match makakoo_core::agents::check_path(&slot, &resolved) {
                    Ok(()) => Authority::Slot(slot_id),
                    Err(error) => {
                        // `allowed_paths` is not the slot's only authority: a
                        // grant bound to this slot extends it at runtime,
                        // which is what `bound_to_agent` exists for. Without
                        // this the reactive-grant flow (USER_GRANTS §12
                        // Flow 1) is impossible for an agent — it would have
                        // to have its spec edited and be restarted.
                        //
                        // Machine-global grants deliberately do NOT apply: an
                        // operator granting themselves a path must not
                        // silently widen every running agent.
                        let slot_grant = utf8_path(&resolved)
                            .and_then(|path| self.matching_grant(path, Some(&slot_id)));
                        match slot_grant {
                            // A forbidden path is a veto no grant overrides.
                            Some(id) if !is_forbidden_for(&slot, &resolved) => {
                                Authority::SlotGrant(id)
                            }
                            _ => {
                                self.emit_audit(raw_path, AuditResult::Denied, None, None);
                                return Err(RpcError::invalid_params(error.to_string()));
                            }
                        }
                    }
                }
            }
            None => match self.authorize_unscoped(&resolved) {
                Ok(authority) => authority,
                Err(message) => {
                    self.emit_audit(raw_path, AuditResult::Denied, None, None);
                    // Deliberately NOT an RpcError: per USER_GRANTS §12 this
                    // is the opening line of a consent conversation, and an
                    // error frame invites the model to retry rather than ask.
                    return Ok(json!({ "ok": false, "error": message }));
                }
            },
        };

        // Parent directories are created inside `write_atomic_nofollow`'s
        // guarded walk. Calling `create_dir_all` here would resolve the path
        // by name and reopen the intermediate-symlink hole that walk closes.
        if let Err(error) =
            makakoo_core::agents::write_atomic_nofollow(&resolved, content.as_bytes())
        {
            // A write that was authorised and then failed is a distinct
            // outcome from one that was refused, and both belong in the log.
            self.emit_audit(raw_path, AuditResult::Error, Some(&authority), None);
            return Err(RpcError::internal(format!(
                "write {}: {error}",
                resolved.display()
            )));
        }

        self.emit_audit(
            raw_path,
            AuditResult::Allowed,
            Some(&authority),
            Some(content.len() as u64),
        );
        Ok(json!({
            "ok": true,
            "path": resolved.to_string_lossy(),
            "bytes": content.len(),
            "authorized_by": authority.audit_scope(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::AGENT_ID;
    use tempfile::TempDir;

    /// A scratch directory that is NOT under the system temp dir.
    ///
    /// `/tmp` is itself a baseline root (`spec/USER_GRANTS.md` §12), so a
    /// fake `$MAKAKOO_HOME` created by `TempDir::new()` lands *inside*
    /// the sandbox on Linux and every "must be refused" case is
    /// legitimately allowed. On macOS the same call returns
    /// `/var/folders/...`, outside `/tmp`, so these tests passed there by
    /// pure accident of platform. Anchor the scratch tree in `target/`
    /// instead, which no baseline root covers.
    fn scratch() -> TempDir {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/test-homes");
        std::fs::create_dir_all(&base).unwrap();
        // Canonicalise the base before handing it to TempDir: the literal
        // `..` above survives into every path built from `tmp.path()`,
        // while the handler canonicalises its home, so the two stop
        // comparing equal and every scope decision inverts.
        let base = base.canonicalize().unwrap();
        TempDir::new_in(&base).unwrap()
    }

    /// A home with the baseline roots materialised, so `resolve_scope_path`
    /// can canonicalise them.
    fn home() -> TempDir {
        let tmp = scratch();
        for sub in ["data/reports", "data/drafts", "tmp", "config"] {
            std::fs::create_dir_all(tmp.path().join(sub)).unwrap();
        }
        tmp
    }

    fn handler(home: &TempDir) -> WriteFileHandler {
        WriteFileHandler::new(Arc::new(ToolContext::empty(
            home.path().canonicalize().unwrap(),
        )))
    }

    /// Write a slot TOML and run `f` with that slot as the current agent.
    async fn as_slot<F, Fut, T>(home: &TempDir, slot_id: &str, toml: &str, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let dir = home.path().join("config/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{slot_id}.toml")), toml).unwrap();
        AGENT_ID.scope(Some(slot_id.to_string()), f()).await
    }

    /// Render a path as a TOML **literal** string.
    ///
    /// A Windows path interpolated into a double-quoted TOML string turns
    /// its backslashes into escape sequences (`C:\\Users` starts an
    /// invalid `\U`), so the slot TOML fails to parse and every scope
    /// decision surfaces as an internal error instead of a refusal.
    /// Literal strings do not process escapes.
    fn tpath(p: &std::path::Path) -> String {
        format!("'{}'", p.display())
    }

    fn slot_toml(slot_id: &str, allowed: &str, forbidden: &str) -> String {
        format!(
            "slot_id = \"{slot_id}\"\n\
             name = \"{slot_id}\"\n\
             inherit_baseline = false\n\
             allowed_paths = [{allowed}]\n\
             forbidden_paths = [{forbidden}]\n\
             tools = [\"write_file\"]\n\
             process_mode = \"supervised_pair\"\n\
             transport = []\n"
        )
    }

    // ── no slot context: baseline + grants ────────────────────────

    #[tokio::test]
    async fn writes_into_a_baseline_root() {
        let home = home();
        let target = home.path().join("data/reports/status.md");
        let out = handler(&home)
            .call(json!({ "path": target.to_string_lossy(), "content": "# status" }))
            .await
            .unwrap();
        assert_eq!(out["ok"], true);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# status");
        assert!(
            out["authorized_by"]
                .as_str()
                .unwrap()
                .starts_with("baseline:"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn rejection_is_a_conversation_not_an_error() {
        // USER_GRANTS §12 Flow 1: the model must be able to quote this back
        // to the user and ask for consent. An RpcError frame invites a retry
        // loop instead of a question.
        let home = home();
        let outside = home.path().join("development/sprints/foo/SPRINT.md");
        let out = handler(&home)
            .call(json!({ "path": outside.to_string_lossy(), "content": "x" }))
            .await
            .expect("refusal must not be an RPC error");
        assert_eq!(out["ok"], false);
        let msg = out["error"].as_str().unwrap();

        assert!(msg.contains("write_file rejected"), "{msg}");
        assert!(msg.contains("Baseline:"), "{msg}");
        assert!(msg.contains("Active grants:"), "{msg}");
        // The quotable part: the exact call that unblocks it, scoped to the
        // parent directory rather than the single file.
        assert!(msg.contains("grant_write_access(path="), "{msg}");
        assert!(msg.contains("duration=\"1h\""), "{msg}");
        assert!(
            msg.contains("development/sprints/foo/"),
            "suggested scope must be the parent dir: {msg}"
        );
        assert!(
            !outside.exists(),
            "a refused write must not create the file"
        );
    }

    #[tokio::test]
    async fn an_active_grant_authorizes_a_path_outside_baseline() {
        use makakoo_core::capability::{new_grant_id, UserGrant};

        let home = home();
        let project = home.path().join("development");
        std::fs::create_dir_all(&project).unwrap();
        let canon = project.canonicalize().unwrap();

        let mut grants = UserGrants::load(home.path());
        let now = Utc::now();
        grants.add(UserGrant {
            id: new_grant_id(now),
            scope: format!("fs/write:{}/**", canon.display()),
            created_at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
            label: "t3.1".into(),
            granted_by: "test".into(),
            plugin: "test".into(),
            origin_turn_id: String::new(),
            owner: "test".into(),
            bound_to_agent: None,
        });
        grants.save().unwrap();

        let target = canon.join("sprints/foo/SPRINT.md");
        let out = handler(&home)
            .call(json!({ "path": target.to_string_lossy(), "content": "progress" }))
            .await
            .unwrap();
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "progress");
        assert!(
            out["authorized_by"].as_str().unwrap().starts_with("g_"),
            "a granted write must be attributed to the grant: {out}"
        );
    }

    // ── slot context: the slot's own scope ────────────────────────

    #[tokio::test]
    async fn a_slot_writes_inside_its_allowed_paths() {
        let home = home();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let canon = work.canonicalize().unwrap();
        let toml = slot_toml("scribe", &tpath(&canon), "");
        let target = canon.join("notes.md");

        let out = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "note" }))
                .await
        })
        .await
        .unwrap();

        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "note");
        assert_eq!(out["authorized_by"], "slot:scribe");
    }

    #[tokio::test]
    async fn a_slot_cannot_write_outside_its_scope_even_into_a_baseline_root() {
        // The baseline roots belong to the operator, not to scoped agents.
        // A slot inheriting them would silently widen every agent's reach.
        let home = home();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let toml = slot_toml("scribe", &tpath(&work.canonicalize().unwrap()), "");
        let target = home.path().join("data/reports/escape.md");

        let error = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "nope" }))
                .await
        })
        .await
        .unwrap_err();

        assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn forbidden_paths_beat_allowed_paths_for_a_slot() {
        let home = home();
        let work = home.path().join("work");
        std::fs::create_dir_all(work.join("secrets")).unwrap();
        let canon = work.canonicalize().unwrap();
        let toml = slot_toml("scribe", &tpath(&canon), &tpath(&canon.join("secrets")));
        let target = canon.join("secrets/keys.md");

        let error = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "nope" }))
                .await
        })
        .await
        .unwrap_err();

        assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn a_grant_bound_to_the_slot_extends_its_allowed_paths() {
        // `bound_to_agent` exists so an agent can be granted a path at
        // runtime. Without this the reactive-grant flow is impossible for an
        // agent: it would need its spec edited and to be restarted.
        use makakoo_core::capability::{new_grant_id, UserGrant};

        let home = home();
        let work = home.path().join("work");
        let extra = home.path().join("extra");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let extra_canon = extra.canonicalize().unwrap();

        let now = Utc::now();
        let mut grants = UserGrants::load(home.path());
        grants.add(UserGrant {
            id: new_grant_id(now),
            scope: format!("fs/write:{}/**", extra_canon.display()),
            created_at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
            label: "slot-bound".into(),
            granted_by: "test".into(),
            plugin: "test".into(),
            origin_turn_id: String::new(),
            owner: "test".into(),
            bound_to_agent: Some("scribe".into()),
        });
        grants.save().unwrap();

        let toml = slot_toml("scribe", &tpath(&work.canonicalize().unwrap()), "");
        let target = extra_canon.join("granted.md");
        let out = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "ok" }))
                .await
        })
        .await
        .unwrap();

        assert_eq!(out["ok"], true, "{out}");
        assert!(
            out["authorized_by"].as_str().unwrap().starts_with("g_"),
            "must be attributed to the grant, not the slot: {out}"
        );
    }

    #[tokio::test]
    async fn a_machine_global_grant_does_not_widen_a_slot() {
        // An operator granting themselves a path must not silently widen
        // every running agent.
        use makakoo_core::capability::{new_grant_id, UserGrant};

        let home = home();
        let work = home.path().join("work");
        let extra = home.path().join("extra");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let extra_canon = extra.canonicalize().unwrap();

        let now = Utc::now();
        let mut grants = UserGrants::load(home.path());
        grants.add(UserGrant {
            id: new_grant_id(now),
            scope: format!("fs/write:{}/**", extra_canon.display()),
            created_at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
            label: "machine-global".into(),
            granted_by: "test".into(),
            plugin: "test".into(),
            origin_turn_id: String::new(),
            owner: "test".into(),
            bound_to_agent: None,
        });
        grants.save().unwrap();

        let toml = slot_toml("scribe", &tpath(&work.canonicalize().unwrap()), "");
        let target = extra_canon.join("nope.md");
        let error = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "nope" }))
                .await
        })
        .await
        .unwrap_err();

        assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn a_slot_bound_grant_cannot_override_forbidden_paths() {
        use makakoo_core::capability::{new_grant_id, UserGrant};

        let home = home();
        let work = home.path().join("work");
        std::fs::create_dir_all(work.join("secrets")).unwrap();
        let canon = work.canonicalize().unwrap();
        let secrets = canon.join("secrets");

        let now = Utc::now();
        let mut grants = UserGrants::load(home.path());
        grants.add(UserGrant {
            id: new_grant_id(now),
            scope: format!("fs/write:{}/**", secrets.display()),
            created_at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
            label: "over-broad".into(),
            granted_by: "test".into(),
            plugin: "test".into(),
            origin_turn_id: String::new(),
            owner: "test".into(),
            bound_to_agent: Some("scribe".into()),
        });
        grants.save().unwrap();

        let toml = slot_toml("scribe", &tpath(&canon), &tpath(&secrets));
        let target = secrets.join("keys.md");
        let error = as_slot(&home, "scribe", &toml, || async {
            handler(&home)
                .call(json!({ "path": target.to_string_lossy(), "content": "leak" }))
                .await
        })
        .await
        .unwrap_err();

        assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS);
        assert!(
            !target.exists(),
            "forbidden_paths must veto a slot-bound grant"
        );
    }

    // ── path safety ───────────────────────────────────────────────

    #[tokio::test]
    #[cfg(unix)]
    async fn a_symlink_out_of_the_sandbox_is_refused_not_followed() {
        // The raw-argument gate in server.rs sees only the literal string; a
        // link inside a baseline root is what makes the resolved-path check
        // here load-bearing rather than redundant.
        let home = home();
        let outside = scratch();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "untouched").unwrap();

        let link = home.path().join("data/reports/link.md");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let out = handler(&home)
            .call(json!({ "path": link.to_string_lossy(), "content": "pwned" }))
            .await
            .unwrap();

        // Authorisation runs on the *resolved* path, so the link is judged by
        // where it points, not where it sits. The target is outside every
        // baseline root, so the write is refused before it is attempted.
        assert_eq!(out["ok"], false, "{out}");
        assert!(
            out["error"].as_str().unwrap().contains("secret.txt"),
            "the rejection must name the resolved target, not the link: {out}"
        );
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "untouched",
            "write escaped the sandbox through a symlink"
        );
        // Defence in depth: even had this been authorised, the O_NOFOLLOW
        // write would have replaced the link rather than followed it — see
        // makakoo-core `safe_write::replaces_a_symlink_instead_of_writing_through_it`.
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn a_symlink_within_the_sandbox_is_written_through_its_target() {
        // The mirror case: a link whose target is *also* inside a baseline
        // root is legitimate, and refusing it would break ordinary use.
        let home = home();
        let real = home.path().join("data/drafts/real.md");
        std::fs::write(&real, "old").unwrap();
        let link = home.path().join("data/reports/alias.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let out = handler(&home)
            .call(json!({ "path": link.to_string_lossy(), "content": "new" }))
            .await
            .unwrap();
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn a_path_through_an_intermediate_symlink_is_refused() {
        // The escape found in review: O_NOFOLLOW guards only the final
        // component, so a symlink *above* the parent used to redirect the
        // whole write. Two layers stop it now — resolution judges the path by
        // where it actually points, and the guarded walk refuses a symlink at
        // any component if the tree changes afterwards.
        let home = home();
        let outside = scratch();
        std::fs::create_dir_all(outside.path().join("parent")).unwrap();
        let link = home.path().join("data/reports/mid");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        let out = handler(&home)
            .call(json!({
                "path": link.join("parent/pwned.md").to_string_lossy(),
                "content": "ESCAPED"
            }))
            .await
            .unwrap();

        assert_eq!(out["ok"], false, "{out}");
        assert!(
            !outside.path().join("parent/pwned.md").exists(),
            "write escaped through an intermediate symlink"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn a_traversal_escape_from_a_baseline_root_is_refused() {
        let home = home();
        let target = home.path().join("data/reports/../../escaped.md");
        let out = handler(&home)
            .call(json!({ "path": target.to_string_lossy(), "content": "x" }))
            .await
            .unwrap();
        assert_eq!(out["ok"], false, "`..` must not climb out of a root: {out}");
        assert!(!home.path().join("escaped.md").exists());
    }

    #[tokio::test]
    async fn a_sibling_directory_does_not_match_a_baseline_root_by_prefix() {
        // "/…/tmp-evil" starts with "/…/tmp" as a *string*. Component-wise
        // comparison is what stops that from being authorised.
        let home = home();
        let evil = home.path().join("tmp-evil");
        std::fs::create_dir_all(&evil).unwrap();
        let out = handler(&home)
            .call(json!({ "path": evil.join("x.md").to_string_lossy(), "content": "x" }))
            .await
            .unwrap();
        assert_eq!(out["ok"], false, "{out}");
    }

    // ── limits + params ───────────────────────────────────────────

    #[tokio::test]
    async fn oversize_content_is_refused_before_anything_is_written() {
        let home = home();
        let target = home.path().join("data/reports/big.md");
        let error = handler(&home)
            .call(json!({
                "path": target.to_string_lossy(),
                "content": "x".repeat(makakoo_core::agents::MAX_WRITE_BYTES + 1)
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn missing_or_empty_params_are_rejected() {
        let home = home();
        for params in [
            json!({}),
            json!({ "path": "/tmp/x.md" }),
            json!({ "content": "x" }),
            json!({ "path": "   ", "content": "x" }),
        ] {
            let error = handler(&home).call(params.clone()).await.unwrap_err();
            assert_eq!(error.code, crate::jsonrpc::INVALID_PARAMS, "{params}");
        }
    }

    #[tokio::test]
    async fn the_path_parameter_is_named_path_so_the_upstream_gate_sees_it() {
        // server.rs::authorize_tool only scope-checks filesystem-shaped
        // argument keys. Renaming this parameter would silently disable the
        // outer half of the sandbox, and nothing else would fail.
        let home = home();
        let schema = handler(&home).input_schema();
        assert!(schema["properties"]["path"].is_object(), "{schema}");
        assert_eq!(schema["required"], json!(["path", "content"]));
    }

    // ── audit ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn both_outcomes_leave_an_fs_write_audit_line() {
        let home = home();
        handler(&home)
            .call(json!({
                "path": home.path().join("data/reports/ok.md").to_string_lossy(),
                "content": "yes"
            }))
            .await
            .unwrap();
        handler(&home)
            .call(json!({
                "path": home.path().join("nope/denied.md").to_string_lossy(),
                "content": "no"
            }))
            .await
            .unwrap();

        let log = AuditLog::open_default(home.path()).unwrap();
        let raw = std::fs::read_to_string(log.path()).unwrap_or_default();
        let entries: Vec<Value> = raw
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|entry: &Value| entry["verb"] == "fs/write")
            .collect();
        assert_eq!(entries.len(), 2, "{raw}");
        assert!(entries.iter().any(|e| e["result"] == "allowed"));
        assert!(entries.iter().any(|e| e["result"] == "denied"));
    }
}
