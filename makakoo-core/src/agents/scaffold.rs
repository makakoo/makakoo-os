//! Agent scaffold — filesystem-level CRUD for agents living under
//! `{MAKAKOO_HOME}/agents/<name>/`.
//!
//! No external crates are pulled in for this port — the canonical
//! `agent.toml` file uses a deliberately restricted "flat key = value"
//! subset (string, u32, datetime as rfc3339) which a ~40-line
//! hand-rolled parser handles. That keeps the Cargo.toml change
//! surface at zero for T11.
//!
//! Operations:
//!
//! * `list()` — scan agents_dir, return parsed specs (unparseable
//!   agents surface as a minimal stub).
//! * `info(name)` — fetch one spec or `None`.
//! * `create(name, kind, description)` — scaffold a new agent with
//!   `agent.toml`, `README.md`, and a stub entry file. Rejects
//!   duplicates.
//! * `install(src_dir)` — validate an incoming agent dir has an
//!   `agent.toml`, reject duplicates, copy into `agents_dir`.
//! * `uninstall(name)` — remove the dir. Safety: if any file in the
//!   dir holds an `fs2` exclusive lock (meaning a running process is
//!   writing to it), fail with an error.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

/// Agent execution kind. Determines which stub entry file `create()`
/// generates and how `install()` validates an incoming bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Python,
    Rust,
    Shell,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Python => "python",
            AgentKind::Rust => "rust",
            AgentKind::Shell => "shell",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "python" | "py" => Ok(AgentKind::Python),
            "rust" | "rs" => Ok(AgentKind::Rust),
            "shell" | "sh" | "bash" => Ok(AgentKind::Shell),
            other => Err(MakakooError::internal(format!(
                "agent kind '{other}' not one of python|rust|shell"
            ))),
        }
    }

    fn stub_entry_file(&self) -> (&'static str, &'static str) {
        match self {
            AgentKind::Python => (
                "run.py",
                "#!/usr/bin/env python3\n\
                 \"\"\"Agent entry point — fill in.\"\"\"\n\n\
                 def main():\n    pass\n\n\
                 if __name__ == \"__main__\":\n    main()\n",
            ),
            AgentKind::Rust => (
                "Cargo.toml",
                "[package]\nname = \"agent\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            AgentKind::Shell => (
                "run.sh",
                "#!/usr/bin/env bash\nset -euo pipefail\n# agent entry point — fill in\n",
            ),
        }
    }

    fn default_entry(&self) -> &'static str {
        match self {
            AgentKind::Python => "run.py",
            AgentKind::Rust => "cargo run --release",
            AgentKind::Shell => "run.sh",
        }
    }
}

/// Canonical agent spec. Round-trips through `agent.toml`. Matches
/// the shape the Python scaffolder persists, modulo a couple of Rust
/// ergonomic additions (`patrol_interval_min`, explicit `version`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSpec {
    pub name: String,
    pub kind: String,
    pub entry: String,
    pub description: String,
    pub version: String,
    pub created_at: DateTime<Utc>,
    pub maintainer: Option<String>,
    pub patrol_interval_min: Option<u32>,
}

impl AgentSpec {
    fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("name = \"{}\"\n", escape(&self.name)));
        out.push_str(&format!("kind = \"{}\"\n", escape(&self.kind)));
        out.push_str(&format!("entry = \"{}\"\n", escape(&self.entry)));
        out.push_str(&format!("description = \"{}\"\n", escape(&self.description)));
        out.push_str(&format!("version = \"{}\"\n", escape(&self.version)));
        out.push_str(&format!(
            "created_at = \"{}\"\n",
            self.created_at.to_rfc3339()
        ));
        if let Some(m) = &self.maintainer {
            out.push_str(&format!("maintainer = \"{}\"\n", escape(m)));
        }
        if let Some(n) = self.patrol_interval_min {
            out.push_str(&format!("patrol_interval_min = {n}\n"));
        }
        out
    }

    fn from_toml(content: &str) -> Result<Self> {
        let mut name = None;
        let mut kind = None;
        let mut entry = None;
        let mut description = None;
        let mut version = None;
        let mut created_at: Option<DateTime<Utc>> = None;
        let mut maintainer = None;
        let mut patrol_interval_min: Option<u32> = None;

        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let (key, value) = match line.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            let key = key.trim();
            let value = value.trim();

            if let Some(v) = parse_string(value) {
                match key {
                    "name" => name = Some(v),
                    "kind" => kind = Some(v),
                    "entry" => entry = Some(v),
                    "description" => description = Some(v),
                    "version" => version = Some(v),
                    "maintainer" => maintainer = Some(v),
                    "created_at" => {
                        created_at = DateTime::parse_from_rfc3339(&v)
                            .ok()
                            .map(|d| d.with_timezone(&Utc));
                    }
                    _ => {}
                }
            } else if key == "patrol_interval_min" {
                patrol_interval_min = value.parse().ok();
            }
        }

        Ok(AgentSpec {
            name: name.ok_or_else(|| MakakooError::internal("agent.toml: missing name"))?,
            kind: kind.unwrap_or_else(|| "python".to_string()),
            entry: entry.unwrap_or_else(|| "run.py".to_string()),
            description: description.unwrap_or_default(),
            version: version.unwrap_or_else(|| "0.1.0".to_string()),
            created_at: created_at.unwrap_or_else(Utc::now),
            maintainer,
            patrol_interval_min,
        })
    }
}

fn parse_string(value: &str) -> Option<String> {
    if value.len() < 2 {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return None;
    }
    Some(unescape(&value[1..value.len() - 1]))
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Filesystem-bound scaffold engine. Cheap to construct.
#[derive(Clone, Debug)]
pub struct AgentScaffold {
    agents_dir: PathBuf,
}

impl AgentScaffold {
    pub fn new(agents_dir: PathBuf) -> Self {
        Self { agents_dir }
    }

    pub fn agents_dir(&self) -> &Path {
        &self.agents_dir
    }

    /// List every agent directory with a parseable `agent.toml`.
    /// Directories without an `agent.toml` are returned as minimal
    /// stubs so the caller can flag them.
    pub fn list(&self) -> Result<Vec<AgentSpec>> {
        if !self.agents_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.agents_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(s) if !s.starts_with('.') => s.to_string(),
                _ => continue,
            };
            let spec_path = path.join("agent.toml");
            if spec_path.exists() {
                match fs::read_to_string(&spec_path).map_err(MakakooError::from).and_then(|c| AgentSpec::from_toml(&c)) {
                    Ok(spec) => out.push(spec),
                    Err(_) => out.push(stub_spec(&name)),
                }
            } else {
                out.push(stub_spec(&name));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn info(&self, name: &str) -> Result<Option<AgentSpec>> {
        let spec_path = self.agents_dir.join(name).join("agent.toml");
        if !spec_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&spec_path)?;
        Ok(Some(AgentSpec::from_toml(&content)?))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.agents_dir.join(name).is_dir()
    }

    /// Scaffold a new agent directory. Rejects if `name` is empty,
    /// contains path separators, or already exists.
    pub fn create(&self, name: &str, kind: &str, description: &str) -> Result<AgentSpec> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            return Err(MakakooError::internal(format!(
                "agent create: invalid name '{name}'"
            )));
        }
        if self.exists(name) {
            return Err(MakakooError::internal(format!(
                "agent create: '{name}' already exists"
            )));
        }

        let kind_enum = AgentKind::parse(kind)?;
        let target = self.agents_dir.join(name);
        fs::create_dir_all(&target)?;

        let spec = AgentSpec {
            name: name.to_string(),
            kind: kind_enum.as_str().to_string(),
            entry: kind_enum.default_entry().to_string(),
            description: description.to_string(),
            version: "0.1.0".to_string(),
            created_at: Utc::now(),
            maintainer: None,
            patrol_interval_min: None,
        };

        // agent.toml
        fs::write(target.join("agent.toml"), spec.to_toml())?;

        // README.md
        fs::write(
            target.join("README.md"),
            format!(
                "# {name}\n\n{description}\n\nKind: {}\nEntry: {}\n",
                kind_enum.as_str(),
                kind_enum.default_entry()
            ),
        )?;

        // Stub entry file.
        let (entry_name, stub_body) = kind_enum.stub_entry_file();
        fs::write(target.join(entry_name), stub_body)?;

        Ok(spec)
    }

    /// Copy an agent directory into the scaffold. The source must
    /// contain a readable `agent.toml`; the `name` field in that file
    /// determines the destination. Rejects duplicates.
    pub fn install(&self, src_dir: &Path) -> Result<AgentSpec> {
        if !src_dir.is_dir() {
            return Err(MakakooError::internal(format!(
                "agent install: source '{}' is not a directory",
                src_dir.display()
            )));
        }
        let spec_path = src_dir.join("agent.toml");
        if !spec_path.exists() {
            return Err(MakakooError::internal(
                "agent install: source missing agent.toml",
            ));
        }
        let content = fs::read_to_string(&spec_path)?;
        let spec = AgentSpec::from_toml(&content)?;
        if spec.name.is_empty() || spec.name.contains('/') || spec.name.contains('\\') {
            return Err(MakakooError::internal(format!(
                "agent install: invalid name '{}' in agent.toml",
                spec.name
            )));
        }
        if self.exists(&spec.name) {
            return Err(MakakooError::internal(format!(
                "agent install: '{}' already installed",
                spec.name
            )));
        }
        let dest = self.agents_dir.join(&spec.name);
        fs::create_dir_all(&dest)?;
        copy_dir_recursive(src_dir, &dest)?;
        Ok(spec)
    }

    /// Remove an agent directory. Safety: if any file in the target
    /// currently holds an exclusive fs2 lock (i.e. a running agent
    /// process is writing to it), the call fails and nothing is
    /// deleted. A missing agent is a clean no-op error.
    pub fn uninstall(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.contains('/') {
            return Err(MakakooError::internal(format!(
                "agent uninstall: invalid name '{name}'"
            )));
        }
        let target = self.agents_dir.join(name);
        if !target.exists() {
            return Err(MakakooError::internal(format!(
                "agent uninstall: '{name}' not found"
            )));
        }

        if dir_is_locked(&target)? {
            return Err(MakakooError::internal(format!(
                "agent uninstall: '{name}' is currently locked (running?)"
            )));
        }

        fs::remove_dir_all(&target)?;
        Ok(())
    }
}

// ─── helpers ────────────────────────────────────────────────────────

fn stub_spec(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        kind: "unknown".to_string(),
        entry: String::new(),
        description: "(no agent.toml)".to_string(),
        version: "0.0.0".to_string(),
        created_at: Utc::now(),
        maintainer: None,
        patrol_interval_min: None,
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let sub_src = entry.path();
        let sub_dst = dst.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir_all(&sub_dst)?;
            copy_dir_recursive(&sub_src, &sub_dst)?;
        } else if file_type.is_file() {
            fs::copy(&sub_src, &sub_dst)?;
        }
        // Symlinks: ignore for now — the Python impl symlinks local
        // source paths, but a fresh install should copy, not link.
    }
    Ok(())
}

/// Return `Ok(true)` if any regular file under `dir` currently holds
/// an exclusive advisory lock. We try to grab a shared lock on each
/// file non-blockingly via `std::fs::File::try_lock_shared` — if it
/// fails with `TryLockError::WouldBlock`, somebody else has it locked
/// exclusive. Any other error is treated as "not locked" so transient
/// unreadable files don't stop an uninstall.
fn dir_is_locked(dir: &Path) -> Result<bool> {
    for entry in walk_files(dir)? {
        let f = match fs::OpenOptions::new().read(true).open(&entry) {
            Ok(f) => f,
            Err(_) => continue,
        };
        match f.try_lock_shared() {
            Ok(()) => {
                let _ = f.unlock();
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Ok(true);
            }
            Err(_) => {
                // Other errors — treat as not-locked so transient fs
                // hiccups don't stop an uninstall.
            }
        }
    }
    Ok(false)
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let rd = match fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(path),
                Ok(ft) if ft.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold() -> (tempfile::TempDir, AgentScaffold) {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        (dir, AgentScaffold::new(agents))
    }

    #[test]
    fn create_scaffolds_python_agent_dir() {
        let (_d, s) = scaffold();
        let spec = s.create("weather-bot", "python", "weather alerts").unwrap();
        assert_eq!(spec.name, "weather-bot");
        assert_eq!(spec.kind, "python");
        assert!(s.exists("weather-bot"));
        assert!(s.agents_dir().join("weather-bot/agent.toml").exists());
        assert!(s.agents_dir().join("weather-bot/README.md").exists());
        assert!(s.agents_dir().join("weather-bot/run.py").exists());
    }

    #[test]
    fn create_rejects_duplicates() {
        let (_d, s) = scaffold();
        s.create("dup", "shell", "").unwrap();
        let err = s.create("dup", "shell", "").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn list_returns_created_agents_sorted() {
        let (_d, s) = scaffold();
        s.create("zeta", "python", "").unwrap();
        s.create("alpha", "python", "").unwrap();
        let list = s.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "zeta");
    }

    #[test]
    fn info_returns_none_for_missing_agent() {
        let (_d, s) = scaffold();
        assert!(s.info("nope").unwrap().is_none());
        s.create("here", "python", "real").unwrap();
        let got = s.info("here").unwrap().unwrap();
        assert_eq!(got.description, "real");
    }

    #[test]
    fn install_from_source_dir_copies_files_and_rejects_duplicate() {
        let (d, s) = scaffold();
        let src = d.path().join("incoming");
        fs::create_dir_all(&src).unwrap();
        let spec = AgentSpec {
            name: "imported".to_string(),
            kind: "python".to_string(),
            entry: "run.py".to_string(),
            description: "from tarball".to_string(),
            version: "1.2.3".to_string(),
            created_at: Utc::now(),
            maintainer: Some("Harvey".to_string()),
            patrol_interval_min: Some(15),
        };
        fs::write(src.join("agent.toml"), spec.to_toml()).unwrap();
        fs::write(src.join("run.py"), "print('hi')\n").unwrap();
        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("subdir/data.txt"), "x\n").unwrap();

        let got = s.install(&src).unwrap();
        assert_eq!(got.name, "imported");
        assert_eq!(got.version, "1.2.3");
        assert_eq!(got.patrol_interval_min, Some(15));
        assert!(s.agents_dir().join("imported/run.py").exists());
        assert!(s.agents_dir().join("imported/subdir/data.txt").exists());

        // Duplicate rejection.
        assert!(s.install(&src).is_err());
    }

    #[test]
    fn install_rejects_source_without_manifest() {
        let (d, s) = scaffold();
        let src = d.path().join("bad");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("run.py"), "pass\n").unwrap();
        let err = s.install(&src).unwrap_err();
        assert!(err.to_string().contains("missing agent.toml"));
    }

    #[test]
    fn uninstall_removes_agent_dir() {
        let (_d, s) = scaffold();
        s.create("temp", "python", "").unwrap();
        assert!(s.exists("temp"));
        s.uninstall("temp").unwrap();
        assert!(!s.exists("temp"));
    }

    #[test]
    fn uninstall_missing_agent_is_an_error() {
        let (_d, s) = scaffold();
        assert!(s.uninstall("ghost").is_err());
    }

    #[test]
    fn agent_spec_round_trips_through_toml() {
        let spec = AgentSpec {
            name: "roundtrip".to_string(),
            kind: "python".to_string(),
            entry: "run.py".to_string(),
            description: "with \"quotes\" and \\ slashes".to_string(),
            version: "0.1.0".to_string(),
            created_at: Utc::now(),
            maintainer: Some("Harvey".to_string()),
            patrol_interval_min: Some(30),
        };
        let serialized = spec.to_toml();
        let parsed = AgentSpec::from_toml(&serialized).unwrap();
        assert_eq!(parsed.name, spec.name);
        assert_eq!(parsed.description, spec.description);
        assert_eq!(parsed.maintainer, spec.maintainer);
        assert_eq!(parsed.patrol_interval_min, spec.patrol_interval_min);
    }
}
