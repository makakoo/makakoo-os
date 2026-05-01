//! Strategy loader.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1 §1 ships 5 canonical strategies as
//! markdown files at `data/strategies/`. They are baked into the
//! binary via `include_str!` so a fresh install gets them with no
//! extra steps. Sebastian-side overrides at
//! `$MAKAKOO_HOME/data/strategies/<name>.md` win when present —
//! cheap customization without recompile.

use std::path::PathBuf;

use thiserror::Error;

use crate::platform::data_dir;

/// Canonical bundled strategies. Names are kebab-case to match
/// `pattern.toml` conventions and the CLI flag (`--strategy cot`).
pub const BUILTIN_STRATEGIES: &[(&str, &str)] = &[
    ("cot", include_str!("../../../data/strategies/cot.md")),
    ("tot", include_str!("../../../data/strategies/tot.md")),
    ("react", include_str!("../../../data/strategies/react.md")),
    (
        "harvey-rigor",
        include_str!("../../../data/strategies/harvey-rigor.md"),
    ),
    (
        "caveman",
        include_str!("../../../data/strategies/caveman.md"),
    ),
];

#[derive(Debug, Error)]
pub enum StrategyLoadError {
    #[error("unknown strategy {name:?} (built-ins: {builtins})")]
    Unknown { name: String, builtins: String },

    #[error("failed to read override file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve a strategy by name. Search order:
///   1. `$MAKAKOO_HOME/data/strategies/<name>.md` (user override)
///   2. Built-in `BUILTIN_STRATEGIES` table baked into the binary
///
/// Returns [`StrategyLoadError::Unknown`] if neither path holds the
/// strategy. Pass `home_override` to point at a different home dir
/// (used by tests).
pub fn load_strategy(
    name: &str,
    home_override: Option<&std::path::Path>,
) -> Result<String, StrategyLoadError> {
    // Step 1: user override.
    let home = home_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(data_dir);
    let override_path = if home_override.is_some() {
        home.join("strategies").join(format!("{name}.md"))
    } else {
        home.join("strategies").join(format!("{name}.md"))
    };
    if override_path.exists() {
        return std::fs::read_to_string(&override_path).map_err(|source| {
            StrategyLoadError::Io {
                path: override_path,
                source,
            }
        });
    }

    // Step 2: built-in.
    for (n, body) in BUILTIN_STRATEGIES {
        if *n == name {
            return Ok(body.to_string());
        }
    }

    Err(StrategyLoadError::Unknown {
        name: name.to_string(),
        builtins: BUILTIN_STRATEGIES
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_each_builtin_strategy() {
        for (name, _) in BUILTIN_STRATEGIES {
            let body = load_strategy(name, Some(std::path::Path::new("/nonexistent")))
                .expect("builtin should resolve");
            assert!(!body.is_empty(), "{name} body empty");
        }
    }

    #[test]
    fn rejects_unknown_strategy() {
        let err = load_strategy("does-not-exist", Some(std::path::Path::new("/nonexistent")))
            .unwrap_err();
        assert!(matches!(err, StrategyLoadError::Unknown { .. }));
    }

    #[test]
    fn override_file_wins_over_builtin() {
        let tmp = TempDir::new().unwrap();
        let strategies_dir = tmp.path().join("strategies");
        std::fs::create_dir_all(&strategies_dir).unwrap();
        std::fs::write(strategies_dir.join("cot.md"), "OVERRIDDEN COT").unwrap();
        let body = load_strategy("cot", Some(tmp.path())).unwrap();
        assert_eq!(body, "OVERRIDDEN COT");
    }

    #[test]
    fn missing_override_falls_back_to_builtin() {
        let tmp = TempDir::new().unwrap();
        // strategies/ dir doesn't exist — builtin should win.
        let body = load_strategy("cot", Some(tmp.path())).unwrap();
        assert!(body.contains("Chain of Thought"));
    }

    #[test]
    fn caveman_directive_includes_hard_gate() {
        let body = load_strategy("caveman", Some(std::path::Path::new("/nonexistent"))).unwrap();
        assert!(
            body.contains("HARD-GATE BYPASS"),
            "caveman.md must surface the HARD-GATE BYPASS preamble at the top"
        );
        assert!(body.contains("TOKEN EFFICIENCY"));
    }

    #[test]
    fn five_strategies_shipped() {
        // Sprint locks the canonical 5: cot, tot, react, harvey-rigor, caveman.
        let names: Vec<&str> = BUILTIN_STRATEGIES.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"cot"));
        assert!(names.contains(&"tot"));
        assert!(names.contains(&"react"));
        assert!(names.contains(&"harvey-rigor"));
        assert!(names.contains(&"caveman"));
    }
}
