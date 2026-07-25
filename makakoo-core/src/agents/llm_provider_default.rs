//! Project-level LLM provider default.
//!
//! Stores the user's preferred `<provider>/<model>` at
//! `$MAKAKOO_HOME/config/llm-default` (or `~/.makakoo/config/llm-default`
//! when `MAKAKOO_HOME` is not set). Read by `makakoo agent create` when
//! the spec's `model` field is empty or just a provider prefix.

use std::path::PathBuf;

const DEFAULT_FILE: &str = "config/llm-default";

/// Resolve the project default LLM specifier. Returns `None` if no
/// default is set or the file is malformed.
pub fn get_default() -> Option<String> {
    let path = default_path()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Set the project default LLM specifier (e.g. `switchailocal/ail-compound`).
/// Writes to `$MAKAKOO_HOME/config/llm-default`.
pub fn set_default(specifier: &str) -> anyhow::Result<()> {
    let path = default_path().ok_or_else(|| {
        anyhow::anyhow!("could not resolve $MAKAKOO_HOME for project default")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, specifier.trim())?;
    Ok(())
}

/// Clear the project default LLM specifier.
pub fn clear_default() -> anyhow::Result<()> {
    let Some(path) = default_path() else { return Ok(()) };
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

fn default_path() -> Option<PathBuf> {
    // 1. $MAKAKOO_HOME wins (project root when set explicitly).
    if let Ok(home) = std::env::var("MAKAKOO_HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home).join(DEFAULT_FILE));
        }
    }
    // 2. Fallback: $HOME or $USERPROFILE (Windows).
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    let p = PathBuf::from(home);
    if p.as_os_str().is_empty() {
        return None;
    }
    Some(p.join(".makakoo").join(DEFAULT_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_default_via_makakoo_home() {
        // Use a temp dir as $MAKAKOO_HOME.
        let tmp = std::env::temp_dir().join(format!(
            "makakoo_llm_default_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var("MAKAKOO_HOME").ok();
        std::env::set_var("MAKAKOO_HOME", &tmp);

        // Initially no default.
        assert!(get_default().is_none());

        // Set and read back.
        set_default("switchailocal/ail-compound").unwrap();
        assert_eq!(get_default().as_deref(), Some("switchailocal/ail-compound"));

        // Trim whitespace on read.
        std::fs::write(tmp.join("config/llm-default"), "  ollama/gemma4:12b  \n").unwrap();
        assert_eq!(get_default().as_deref(), Some("ollama/gemma4:12b"));

        // Clear.
        clear_default().unwrap();
        assert!(get_default().is_none());

        // Restore env.
        match prev {
            Some(v) => std::env::set_var("MAKAKOO_HOME", v),
            None => std::env::remove_var("MAKAKOO_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
