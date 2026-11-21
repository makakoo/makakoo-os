//! `makakoo distro lint` — strict headless capability/policy linter CLI command.

use std::path::PathBuf;

use makakoo_core::distro::{lint_distro_file, LintOptions};
use serde_json::json;

use crate::output;

pub fn run(
    path: PathBuf,
    plugins_root: Option<PathBuf>,
    allow_net: Vec<String>,
    json_out: bool,
) -> anyhow::Result<i32> {
    if !path.is_file() {
        if json_out {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "distro": path.display().to_string(),
                    "ok": false,
                    "violations": [{
                        "plugin": "(distro)",
                        "rule": "missing_distro",
                        "detail": format!("distro file not found: {}", path.display()),
                    }],
                }))?
            );
        } else {
            output::print_error(format!("distro file not found: {}", path.display()));
        }
        return Ok(1);
    }

    let resolved_plugins_root = match plugins_root {
        Some(dir) => dir,
        None => match crate::commands::plugin::plugins_core_root() {
            Ok(dir) => dir,
            Err(e) => {
                // If the distro itself has no plugins (e.g. minimal.toml) and no
                // plugins-core dir is present, fall back to a non-existent path so
                // empty distros pass while any plugin reference still fails closed.
                tracing::debug!(error = %e, "plugins-core not found; falling back to empty root");
                path.parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("plugins-core")
            }
        },
    };

    let opts = LintOptions {
        plugins_root: resolved_plugins_root,
        allowed_net: allow_net,
    };

    let report = match lint_distro_file(&path, &opts) {
        Ok(r) => r,
        Err(e) => {
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "distro": path.display().to_string(),
                        "ok": false,
                        "violations": [{
                            "plugin": "(distro)",
                            "rule": "resolve_error",
                            "detail": e.to_string(),
                        }],
                    }))?
                );
            } else {
                output::print_error(format!("distro lint failed: {e}"));
            }
            return Ok(1);
        }
    };

    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.ok {
        output::print_info(format!(
            "distro {} PASSED strict headless lint ({} plugin(s) checked, 0 violations)",
            report.distro, report.checked_plugins
        ));
    } else {
        output::print_error(format!(
            "distro {} FAILED strict headless lint ({} violation(s) across {} plugin(s)):",
            report.distro,
            report.violations.len(),
            report.checked_plugins
        ));
        for v in &report.violations {
            eprintln!("  - [{}] {}: {}", v.rule.as_str(), v.plugin, v.detail);
        }
    }

    Ok(if report.ok { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cli_distro_lint_minimal_returns_zero_and_core_returns_one() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let minimal = repo_root.join("distros/minimal.toml");
        let core = repo_root.join("distros/core.toml");
        let plugins_root = repo_root.join("plugins-core");
        if !minimal.is_file() || !plugins_root.is_dir() {
            return;
        }
        let rc_min = run(minimal, Some(plugins_root.clone()), vec![], true).unwrap();
        assert_eq!(rc_min, 0);

        let rc_core = run(core, Some(plugins_root), vec![], true).unwrap();
        assert_eq!(rc_core, 1);
    }
}
