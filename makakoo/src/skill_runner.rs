//! Python skill subprocess bridge.
//!
//! Python skills live under `$MAKAKOO_HOME/harvey-os/skills/` as
//! directories containing a `SKILL.md` plus one or more runnable
//! scripts. Rust can't import Python, so the runner spawns `python3`
//! with the skill's entry file and inherits stdio so the user sees
//! live output.
//!
//! Entry-file discovery order:
//!
//!   1. `run.py`
//!   2. `<skill_name>.py`
//!   3. `main.py`
//!   4. The first `.py` file in the skill directory, if any
//!
//! Rust doesn't try to parse SKILL.md frontmatter for an explicit
//! entrypoint yet — every skill the user ships today follows one of
//! the four patterns above. If that changes, extend [`discover`] with
//! a YAML-frontmatter pass that reads an `entry:` key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{anyhow, Context, Result};

use makakoo_core::platform::makakoo_home;

/// A resolved skill pointing at a specific entry file.
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    pub name: String,
    pub skill_dir: PathBuf,
    pub entry: PathBuf,
}

/// Runs Python skills in-process via subprocess.
pub struct SkillRunner {
    python: PathBuf,
    skills_dir: PathBuf,
    env: HashMap<String, String>,
}

impl SkillRunner {
    /// Build a runner rooted at `$MAKAKOO_HOME/harvey-os/skills/`. The
    /// runner inherits the current `PATH` and layers in `PYTHONPATH`,
    /// `MAKAKOO_HOME`, and `HARVEY_HOME` so any skill code that imports
    /// `core.whatever` or reads env vars finds what it expects.
    pub fn new() -> Result<Self> {
        let home = makakoo_home();
        let skills_dir = home.join("harvey-os").join("skills");
        let python = which_python()?;
        let mut env = HashMap::new();
        env.insert("MAKAKOO_HOME".into(), home.to_string_lossy().into_owned());
        env.insert("HARVEY_HOME".into(), home.to_string_lossy().into_owned());
        // Put harvey-os on the Python import path so skills can do
        // `from core.xyz import ...`.
        let pythonpath = home.join("harvey-os").to_string_lossy().into_owned();
        env.insert("PYTHONPATH".into(), pythonpath);
        Ok(Self {
            python,
            skills_dir,
            env,
        })
    }

    /// Construct a runner against an explicit skills dir + python
    /// binary — used by unit tests with a tempdir.
    pub fn with_paths(python: PathBuf, skills_dir: PathBuf) -> Self {
        let mut env = HashMap::new();
        if let Some(parent) = skills_dir.parent() {
            env.insert(
                "MAKAKOO_HOME".into(),
                parent
                    .parent()
                    .unwrap_or(parent)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        Self {
            python,
            skills_dir,
            env,
        }
    }

    /// Return the configured skills dir.
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    /// Locate a skill by name. Walks the skills directory looking for
    /// a subdirectory whose basename matches `name` and contains a
    /// `SKILL.md` file. Returns the first match.
    pub fn discover(&self, name: &str) -> Result<DiscoveredSkill> {
        if !self.skills_dir.exists() {
            return Err(anyhow!(
                "skills dir {} does not exist",
                self.skills_dir.display()
            ));
        }
        let hit = walk_for_skill(&self.skills_dir, name)?;
        let Some(skill_dir) = hit else {
            return Err(anyhow!(
                "skill '{name}' not found under {}",
                self.skills_dir.display()
            ));
        };
        let entry = resolve_entry(&skill_dir, name)
            .with_context(|| format!("no runnable .py entry file under {}", skill_dir.display()))?;
        Ok(DiscoveredSkill {
            name: name.to_string(),
            skill_dir,
            entry,
        })
    }

    /// Run a discovered skill with `args`. Inherits stdio so the user
    /// sees live output. Returns the child's exit status.
    pub fn run(&self, name: &str, args: &[String]) -> Result<ExitStatus> {
        let skill = self.discover(name)?;
        let mut cmd = Command::new(&self.python);
        cmd.arg(&skill.entry).args(args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn python3 for skill '{name}'"))?;
        Ok(status)
    }
}

/// Resolve a `python3` binary. Prefers `$MAKAKOO_PYTHON`, then
/// `python3` on `$PATH`, then `python`.
fn which_python() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("MAKAKOO_PYTHON") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    for candidate in ["python3", "python"] {
        if let Ok(out) = Command::new("which").arg(candidate).output() {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !raw.is_empty() {
                    return Ok(PathBuf::from(raw));
                }
            }
        }
    }
    Ok(PathBuf::from("python3"))
}

/// Walk `dir` recursively looking for a subdirectory named `name`
/// containing a `SKILL.md` file AND at least one runnable `.py`
/// (anything not starting with `_`). Returns the first match.
/// Capped at a reasonable traversal depth to avoid symlink loops.
///
/// The runnable-python predicate matters because some skill names
/// collide across categories — e.g. `meta/health` is docs-only while
/// `system/health` has the actual dashboard. Without the predicate,
/// depth-first traversal can hand back the wrong directory.
fn walk_for_skill(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    fn dir_has_runnable_py(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let basename = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if basename.starts_with('_') {
                continue;
            }
            return true;
        }
        false
    }

    fn inner(dir: &Path, name: &str, depth: usize) -> Result<Option<PathBuf>> {
        if depth > 8 {
            return Ok(None);
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let basename = match path.file_name().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if basename == name
                && path.join("SKILL.md").is_file()
                && dir_has_runnable_py(&path)
            {
                return Ok(Some(path));
            }
            if let Some(found) = inner(&path, name, depth + 1)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
    inner(dir, name, 0)
}

/// Pick the entry `.py` file for a skill. See module docs for the
/// ordered candidate list.
fn resolve_entry(skill_dir: &Path, name: &str) -> Result<PathBuf> {
    let candidates = [
        skill_dir.join("run.py"),
        skill_dir.join(format!("{name}.py")),
        skill_dir.join("main.py"),
    ];
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    // Fallback — first `.py` file in the skill dir whose basename
    // doesn't start with `_`. Skipping the underscore prefix keeps
    // Python conventions (`__init__.py`, private helpers like
    // `_internal.py`) from being picked accidentally — those are
    // never the user-facing entry point.
    if let Ok(entries) = std::fs::read_dir(skill_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let basename = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if basename.starts_with('_') {
                continue;
            }
            return Ok(p);
        }
    }
    Err(anyhow!("no .py entry file in {}", skill_dir.display()))
}

// ─────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn scratch_skills(name: &str, script: &str, body: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let skills_dir = dir.path().join("skills");
        let category_dir = skills_dir.join("meta").join(name);
        fs::create_dir_all(&category_dir).unwrap();
        fs::write(category_dir.join("SKILL.md"), "---\nname: test\n---\n").unwrap();
        fs::write(category_dir.join(script), body).unwrap();
        (dir, skills_dir)
    }

    #[test]
    fn discover_finds_run_py() {
        let (_dir, skills_dir) = scratch_skills("alpha", "run.py", "print('hi')\n");
        let runner = SkillRunner::with_paths(PathBuf::from("python3"), skills_dir);
        let hit = runner.discover("alpha").unwrap();
        assert_eq!(hit.name, "alpha");
        assert!(hit.entry.ends_with("run.py"));
    }

    #[test]
    fn discover_falls_back_to_named_py() {
        let (_dir, skills_dir) = scratch_skills("beta", "beta.py", "print('hi')\n");
        let runner = SkillRunner::with_paths(PathBuf::from("python3"), skills_dir);
        let hit = runner.discover("beta").unwrap();
        assert!(hit.entry.ends_with("beta.py"));
    }

    #[test]
    fn discover_missing_skill_errors() {
        let (_dir, skills_dir) = scratch_skills("alpha", "run.py", "x=1\n");
        let runner = SkillRunner::with_paths(PathBuf::from("python3"), skills_dir);
        let err = runner.discover("gamma").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn discover_recurses_into_subdirs() {
        // Put the skill dir a few levels deep; walk_for_skill should
        // still find it.
        let dir = TempDir::new().unwrap();
        let deep = dir
            .path()
            .join("skills")
            .join("ai-ml")
            .join("diffusion")
            .join("gamma");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("SKILL.md"), "---\nname: gamma\n---\n").unwrap();
        fs::write(deep.join("run.py"), "print('deep')\n").unwrap();
        let runner =
            SkillRunner::with_paths(PathBuf::from("python3"), dir.path().join("skills"));
        let hit = runner.discover("gamma").unwrap();
        assert!(hit.skill_dir.ends_with("gamma"));
    }

    #[test]
    fn run_executes_subprocess() {
        // Use `python3 -c` style via a tiny run.py that exits 0. We
        // rely on python3 being on PATH in the dev env; skip the test
        // cleanly if it isn't.
        let Ok(python) = Command::new("which").arg("python3").output() else {
            return;
        };
        if !python.status.success() {
            return;
        }
        let py = PathBuf::from(
            String::from_utf8_lossy(&python.stdout).trim().to_string(),
        );
        let (_dir, skills_dir) =
            scratch_skills("zeta", "run.py", "import sys; sys.exit(0)\n");
        let runner = SkillRunner::with_paths(py, skills_dir);
        let status = runner.run("zeta", &[]).unwrap();
        assert!(status.success());
    }

    #[test]
    fn run_nonzero_exit_propagates() {
        let Ok(python) = Command::new("which").arg("python3").output() else {
            return;
        };
        if !python.status.success() {
            return;
        }
        let py = PathBuf::from(
            String::from_utf8_lossy(&python.stdout).trim().to_string(),
        );
        let (_dir, skills_dir) =
            scratch_skills("omega", "run.py", "import sys; sys.exit(7)\n");
        let runner = SkillRunner::with_paths(py, skills_dir);
        let status = runner.run("omega", &[]).unwrap();
        assert!(!status.success());
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn resolve_entry_prefers_run_py_order() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(p.join("run.py"), "x=1\n").unwrap();
        fs::write(p.join("gamma.py"), "x=2\n").unwrap();
        fs::write(p.join("main.py"), "x=3\n").unwrap();
        let entry = resolve_entry(p, "gamma").unwrap();
        assert!(entry.ends_with("run.py"));
    }

    #[test]
    fn discover_skips_docs_only_collision() {
        // Two folders named "health": meta/health is docs-only,
        // system/health has the runnable dashboard. Discover must
        // skip the docs-only sibling and return the runnable one.
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");

        let docs_only = skills.join("meta").join("health");
        fs::create_dir_all(&docs_only).unwrap();
        fs::write(docs_only.join("SKILL.md"), "---\nname: health\n---\n").unwrap();

        let runnable = skills.join("system").join("health");
        fs::create_dir_all(&runnable).unwrap();
        fs::write(runnable.join("SKILL.md"), "---\nname: health\n---\n").unwrap();
        fs::write(runnable.join("__init__.py"), "\"\"\"docstring\"\"\"\n").unwrap();
        fs::write(runnable.join("health_dashboard.py"), "print('hi')\n").unwrap();

        let runner = SkillRunner::with_paths(PathBuf::from("python3"), skills);
        let hit = runner.discover("health").unwrap();
        assert!(hit.skill_dir.ends_with("system/health"));
        assert!(hit.entry.ends_with("health_dashboard.py"));
    }

    #[test]
    fn resolve_entry_fallback_skips_underscore_prefix() {
        // Real-world layout: __init__.py + the actual wizard. The
        // fallback must skip __init__.py and pick the wizard, even
        // when the filesystem returns __init__.py first.
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(p.join("__init__.py"), "\"\"\"docstring\"\"\"\n").unwrap();
        fs::write(p.join("file_manager.py"), "print('hi')\n").unwrap();
        let entry = resolve_entry(p, "files").unwrap();
        assert!(entry.ends_with("file_manager.py"));
    }
}
