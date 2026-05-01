//! Mascot persona loader.
//!
//! Loads `plugins-core/mascot-<name>/persona.md` at runtime. Phase 4
//! externalizes Olibia from `plugins/lib-harvey-core/.../mascot.py`
//! and creates skeleton dirs for Pixel/Cinder/Ziggy. The composer
//! reads from disk; nothing is baked in (mascots are user-editable).

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::platform::makakoo_home;

#[derive(Debug, Error)]
pub enum MascotLoadError {
    #[error("mascot {name:?} has no persona — looked at {path}")]
    NotFound { name: String, path: PathBuf },

    #[error("failed to read mascot persona {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Load a mascot persona by name. Resolves to
/// `<plugins_root>/mascot-<name>/persona.md`. The plugins root is
/// derived from `$MAKAKOO_HOME/plugins/` by default; tests pass an
/// override.
pub fn load_mascot(name: &str, plugins_root: Option<&Path>) -> Result<String, MascotLoadError> {
    let root = plugins_root
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_plugins_root);
    let path = root
        .join(format!("mascot-{name}"))
        .join("persona.md");
    if !path.exists() {
        return Err(MascotLoadError::NotFound {
            name: name.to_string(),
            path,
        });
    }
    std::fs::read_to_string(&path).map_err(|source| MascotLoadError::Io { path, source })
}

fn default_plugins_root() -> PathBuf {
    makakoo_home().join("plugins")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_mascot(root: &Path, name: &str, body: &str) {
        let dir = root.join(format!("mascot-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("persona.md"), body).unwrap();
    }

    #[test]
    fn loads_mascot_persona() {
        let tmp = TempDir::new().unwrap();
        seed_mascot(tmp.path(), "olibia", "I am Olibia.");
        let body = load_mascot("olibia", Some(tmp.path())).unwrap();
        assert_eq!(body, "I am Olibia.");
    }

    #[test]
    fn missing_mascot_errors_clearly() {
        let tmp = TempDir::new().unwrap();
        let err = load_mascot("ghost", Some(tmp.path())).unwrap_err();
        assert!(matches!(err, MascotLoadError::NotFound { ref name, .. } if name == "ghost"));
    }

    #[test]
    fn missing_persona_md_in_present_dir_errors() {
        let tmp = TempDir::new().unwrap();
        // Create the dir but no persona.md inside.
        std::fs::create_dir_all(tmp.path().join("mascot-cinder")).unwrap();
        let err = load_mascot("cinder", Some(tmp.path())).unwrap_err();
        assert!(matches!(err, MascotLoadError::NotFound { .. }));
    }
}
