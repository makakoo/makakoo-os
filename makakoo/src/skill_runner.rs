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
//!   0. `entry:` key in SKILL.md YAML frontmatter (highest priority)
//!   1. `run.py`
//!   2. `<skill_name>.py`
//!   3. `main.py`
//!   4. The first `.py` file in the skill directory, if any

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
        let env = build_skill_env(&home, &[]);
        Ok(Self {
            python,
            skills_dir,
            env,
        })
    }

    /// Build a runner with library plugin paths injected into PYTHONPATH.
    pub fn with_library_paths(library_paths: &[PathBuf]) -> Result<Self> {
        let home = makakoo_home();
        let skills_dir = home.join("harvey-os").join("skills");
        let python = which_python()?;
        let env = build_skill_env(&home, library_paths);
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

/// Build the environment map for running Python skills. PYTHONPATH is
/// built from library-plugin src/ dirs + the existing env value.
///
/// `$MAKAKOO_HOME/harvey-os` is NOT on PYTHONPATH by default. Library
/// plugins own the `core.*` namespace via `lib-hte` (core.terminal)
/// and `lib-harvey-core` (core.{security,gym,superbrain,agent,dreams,
/// sancho,chat}). The caller passes library paths from the registry's
/// `get_library_paths()`; each gets `/src` joined so PEP-420
/// namespace-package layouts resolve.
///
/// This is the single source of truth for skill/plugin env setup —
/// both `SkillRunner` and the plugin dispatch bridge use this.
pub fn build_skill_env(home: &Path, library_paths: &[PathBuf]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("MAKAKOO_HOME".into(), home.to_string_lossy().into_owned());
    env.insert("HARVEY_HOME".into(), home.to_string_lossy().into_owned());

    // PYTHONPATH = library plugin src/ dirs + existing env value.
    // Each library plugin's install root is <plugin_root>, with Python
    // packages at <plugin_root>/src/ — so that's the import root.
    // Python silently ignores non-existent PYTHONPATH entries, so a
    // library plugin whose source doesn't nest under src/ costs nothing.
    let mut parts: Vec<PathBuf> = Vec::new();
    for lp in library_paths {
        parts.push(lp.join("src"));
    }
    if let Ok(existing) = std::env::var("PYTHONPATH") {
        if !existing.is_empty() {
            parts.extend(std::env::split_paths(&existing));
        }
    }

    if let Ok(pp) = std::env::join_paths(parts) {
        env.insert("PYTHONPATH".into(), pp.to_string_lossy().into_owned());
    }
    env
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

/// Extract the `entry:` value from SKILL.md YAML frontmatter, if present.
/// Frontmatter is the text between the first `---` line and the next `---`.
fn read_entry_from_frontmatter(skill_dir: &Path) -> Option<String> {
    let skill_md = skill_dir.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md).ok()?;
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find the closing `---` after the opening one.
    let after_open = &trimmed[3..].trim_start_matches(['\r', '\n']);
    let close = after_open.find("\n---")?;
    let frontmatter = &after_open[..close];
    // Simple line-by-line parse for `entry:` — avoids pulling in a
    // YAML library for one key.
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("entry:") {
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Pick the entry `.py` file for a skill. See module docs for the
/// ordered candidate list.
fn resolve_entry(skill_dir: &Path, name: &str) -> Result<PathBuf> {
    // Priority 0: explicit `entry:` in SKILL.md frontmatter.
    if let Some(entry_name) = read_entry_from_frontmatter(skill_dir) {
        let entry_path = skill_dir.join(&entry_name);
        if entry_path.is_file() {
            return Ok(entry_path);
        }
        return Err(anyhow!(
            "SKILL.md declares entry: '{}' but file does not exist in {}",
            entry_name,
            skill_dir.display()
        ));
    }

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
    #[cfg(unix)]
    fn run_executes_subprocess() {
        // Use `python3 -c` style via a tiny run.py that exits 0. We
        // rely on python3 being on PATH in the dev env; skip the test
        // cleanly if it isn't. Windows uses `python` / `py -3` and
        // there's no `which`; a sibling Windows test belongs to
        // Phase H.4 alongside the Windows skill-runner work.
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
    #[cfg(unix)]
    fn run_nonzero_exit_propagates() {
        // Same Unix-only rationale as run_executes_subprocess above.
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

    // ── entry: frontmatter tests ──────────────────────────────────

    #[test]
    fn resolve_entry_uses_frontmatter_entry_key() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(
            p.join("SKILL.md"),
            "---\nname: loops\nentry: loop_runner.py\n---\n# Loops\n",
        )
        .unwrap();
        fs::write(p.join("run.py"), "decoy\n").unwrap();
        fs::write(p.join("loop_runner.py"), "print('loops')\n").unwrap();
        let entry = resolve_entry(p, "loops").unwrap();
        // entry: key wins over run.py
        assert!(entry.ends_with("loop_runner.py"));
    }

    #[test]
    fn resolve_entry_frontmatter_missing_file_errors() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(
            p.join("SKILL.md"),
            "---\nname: ghost\nentry: does_not_exist.py\n---\n",
        )
        .unwrap();
        let err = resolve_entry(p, "ghost").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn resolve_entry_no_frontmatter_falls_through() {
        // SKILL.md without entry: key — should fall through to
        // the normal heuristic cascade.
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(p.join("SKILL.md"), "---\nname: plain\n---\n").unwrap();
        fs::write(p.join("run.py"), "print('hi')\n").unwrap();
        let entry = resolve_entry(p, "plain").unwrap();
        assert!(entry.ends_with("run.py"));
    }

    #[test]
    fn resolve_entry_no_skill_md_falls_through() {
        // No SKILL.md at all — should still work via heuristics.
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(p.join("main.py"), "print('hi')\n").unwrap();
        let entry = resolve_entry(p, "whatever").unwrap();
        assert!(entry.ends_with("main.py"));
    }

    #[test]
    fn resolve_entry_frontmatter_quoted_value() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        fs::write(
            p.join("SKILL.md"),
            "---\nname: quoted\nentry: \"wizard.py\"\n---\n",
        )
        .unwrap();
        fs::write(p.join("wizard.py"), "print('q')\n").unwrap();
        let entry = resolve_entry(p, "quoted").unwrap();
        assert!(entry.ends_with("wizard.py"));
    }

    #[test]
    fn discover_with_frontmatter_entry() {
        // End-to-end: walk_for_skill finds the dir, resolve_entry
        // reads the frontmatter entry: key.
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        let skill_dir = skills.join("meta").join("loops");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: loops\nentry: loop_runner.py\n---\n",
        )
        .unwrap();
        fs::write(skill_dir.join("loop_runner.py"), "print('loops')\n").unwrap();
        fs::write(skill_dir.join("other.py"), "decoy\n").unwrap();
        let runner = SkillRunner::with_paths(PathBuf::from("python3"), skills);
        let hit = runner.discover("loops").unwrap();
        assert!(hit.entry.ends_with("loop_runner.py"));
    }

    // ── build_skill_env tests ─────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn build_env_prepends_not_clobbers() {
        let sentinel = "/usr/lib/existing-path";
        let old = std::env::var("PYTHONPATH").ok();
        std::env::set_var("PYTHONPATH", sentinel);

        let home = PathBuf::from("/fake/home");
        let env = build_skill_env(&home, &[]);
        let pp = env.get("PYTHONPATH").unwrap();
        assert!(pp.contains(sentinel), "existing PYTHONPATH must be preserved");

        match old {
            Some(v) => std::env::set_var("PYTHONPATH", v),
            None => std::env::remove_var("PYTHONPATH"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn build_env_includes_library_paths_as_src_dirs() {
        let old = std::env::var("PYTHONPATH").ok();
        std::env::remove_var("PYTHONPATH");

        let home = PathBuf::from("/fake/home");
        let lib_paths = vec![
            PathBuf::from("/plugins/lib-hte"),
            PathBuf::from("/plugins/lib-harvey-core"),
        ];
        let env = build_skill_env(&home, &lib_paths);
        let pp = env.get("PYTHONPATH").unwrap();
        // Library plugins contribute their src/ subdir so `from
        // core.terminal import ...` resolves against <plugin>/src/core/terminal/.
        assert!(pp.contains("/plugins/lib-hte/src"), "lib-hte should join src/");
        assert!(
            pp.contains("/plugins/lib-harvey-core/src"),
            "lib-harvey-core should join src/"
        );

        if let Some(v) = old {
            std::env::set_var("PYTHONPATH", v);
        }
    }

    /// Phase-3 lock: `build_skill_env` must NOT prepend
    /// `$MAKAKOO_HOME/harvey-os` to PYTHONPATH. The hybrid runtime
    /// where skills imported `from core.*` via the harvey-os submodule
    /// is retired in favour of the lib-hte + lib-harvey-core library
    /// plugins owning the `core.*` namespace.
    ///
    /// Any future regression that re-adds `home.join("harvey-os")`
    /// to the PYTHONPATH build path fails this test loudly, which is
    /// exactly what we want — it shouldn't sneak back in.
    #[test]
    #[cfg(unix)]
    fn build_env_does_not_include_harvey_os_path() {
        let old = std::env::var("PYTHONPATH").ok();
        std::env::remove_var("PYTHONPATH");

        let home = PathBuf::from("/fake/home");
        let env = build_skill_env(&home, &[]);
        let pp = env.get("PYTHONPATH").map(|s| s.as_str()).unwrap_or("");
        assert!(
            !pp.contains("harvey-os"),
            "PYTHONPATH must not reference harvey-os anymore — got {pp:?}"
        );

        // Also holds with library paths present.
        let lib = vec![PathBuf::from("/plugins/lib-hte")];
        let env2 = build_skill_env(&home, &lib);
        let pp2 = env2.get("PYTHONPATH").map(|s| s.as_str()).unwrap_or("");
        assert!(
            !pp2.contains("harvey-os"),
            "PYTHONPATH must stay harvey-os-free even with library paths — got {pp2:?}"
        );

        if let Some(v) = old {
            std::env::set_var("PYTHONPATH", v);
        }
    }
}
