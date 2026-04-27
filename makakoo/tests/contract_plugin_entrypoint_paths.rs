//! Contract test: plugin.toml entrypoint paths must resolve correctly
//! relative to the plugin root.
//!
//! SANCHO's `build_subprocess_handler` (see `makakoo-core/src/sancho/mod.rs`)
//! sets CWD to the plugin root before spawning `[entrypoint].run` and
//! `[entrypoint].start`. A `run` value like `python3 -u plugins/foo/bar.py`
//! double-prefixes — the resolved path becomes
//! `<plugin_root>/plugins/foo/bar.py` which does not exist.
//!
//! Similarly, `agents/` prefixes are a harvey-os-era convention that no
//! longer matches the post-migration plugin layout.
//!
//! This test catches the v0.2 migration-debt regression fixed in v0.5
//! Phase A (`watchdog-infect`, `watchdog-postgres`).

use std::fs;
use std::path::{Path, PathBuf};

/// Any script path token whose first segment matches one of these prefixes
/// indicates a misrooted entrypoint. The subprocess handler CWDs to the
/// plugin root already, so no plugin-relative script should start with
/// these directory names.
const BAD_FIRST_SEGMENTS: &[&str] = &[
    "plugins",
    "plugins-core",
    "agents",
    "harvey-os",
];

/// Walk `plugins-core/<name>/plugin.toml` and collect every `[entrypoint]`
/// command string.
fn collect_entrypoints(repo_root: &Path) -> Vec<(PathBuf, String, String)> {
    let plugins_core = repo_root.join("plugins-core");
    let mut out: Vec<(PathBuf, String, String)> = Vec::new();
    let Ok(entries) = fs::read_dir(&plugins_core) else {
        return out;
    };
    for entry in entries.flatten() {
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }
        let toml_path = plugin_dir.join("plugin.toml");
        let Ok(text) = fs::read_to_string(&toml_path) else {
            continue;
        };
        let mut in_entrypoint = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('[') {
                in_entrypoint = line == "[entrypoint]";
                continue;
            }
            if !in_entrypoint {
                continue;
            }
            for key in ["run", "start", "stop", "health"] {
                let prefix = format!("{key} = \"");
                if let Some(rest) = line.strip_prefix(&prefix) {
                    if let Some(cmd) = rest.strip_suffix('"') {
                        out.push((toml_path.clone(), key.to_string(), cmd.to_string()));
                    }
                }
            }
        }
    }
    out
}

/// Tokenise an entrypoint command string and inspect each argument. Any
/// bare path argument whose first path segment is in `BAD_FIRST_SEGMENTS`
/// fails — it indicates the plugin author duplicated the plugin root
/// prefix under CWD.
fn first_segment_violation(cmd: &str) -> Option<String> {
    // Split on whitespace. `python3 -u <script>` → we care about tokens
    // that look like relative paths (contain `/` or end in `.py`).
    for tok in cmd.split_whitespace() {
        if !tok.contains('/') && !tok.ends_with(".py") && !tok.ends_with(".sh") {
            continue;
        }
        if tok.starts_with('/') || tok.starts_with('$') || tok.starts_with('~') {
            // Absolute / env-var / home paths are the plugin author's
            // explicit choice; contract test only guards plugin-root
            // relatives.
            continue;
        }
        let first = tok.split('/').next().unwrap_or("");
        if BAD_FIRST_SEGMENTS.contains(&first) {
            return Some(tok.to_string());
        }
    }
    None
}

#[test]
fn plugin_entrypoints_have_no_bad_first_segments() {
    let repo_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("makakoo/ sits inside the workspace root")
        .to_path_buf();

    let entrypoints = collect_entrypoints(&repo_root);
    let mut violations: Vec<(PathBuf, String, String, String)> = Vec::new();
    for (toml, key, cmd) in entrypoints {
        if let Some(bad) = first_segment_violation(&cmd) {
            violations.push((toml, key, cmd, bad));
        }
    }

    if !violations.is_empty() {
        let mut msg = String::from(
            "\n\n❌ plugin.toml entrypoint paths start with a prefix that \
             double-nests under the subprocess CWD.\n\n\
             SANCHO CWDs to the plugin root before spawning. Strip the \
             `plugins/`, `plugins-core/`, `agents/`, or `harvey-os/` \
             prefix so the path resolves inside the plugin directly.\n\n",
        );
        for (toml, key, cmd, bad) in &violations {
            msg.push_str(&format!(
                "  {} [entrypoint].{} = {:?} (offending token: {:?})\n",
                toml.strip_prefix(&repo_root).unwrap_or(toml).display(),
                key,
                cmd,
                bad,
            ));
        }
        panic!("{msg}");
    }
}
