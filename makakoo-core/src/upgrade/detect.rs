//! Install-method detector.
//!
//! Inspects the running binary's resolved path and maps it to one of
//! four install methods. The detector is pure (no network) — brew
//! ownership confirmation happens in the dispatcher when needed.

use std::path::{Path, PathBuf};

/// How the running `makakoo` binary was installed. Drives which
/// upgrade dispatcher runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// Installed via `cargo install`. The detector does not yet know
    /// whether the source was crates.io or a local path — that's
    /// resolved per-call (`--source <path>` or `MAKAKOO_SOURCE_PATH`
    /// env override) inside the dispatcher.
    Cargo { source: CargoSource },
    /// Installed via Homebrew (typically `traylinx/tap/makakoo`).
    Homebrew { prefix: PathBuf },
    /// Installed via `curl-pipe` of `install.sh` to `$MAKAKOO_PREFIX/bin/`
    /// (default `$HOME/.local/bin/`).
    CurlPipe { prefix: PathBuf },
    /// None of the above. The CLI prints an actionable error and exits.
    Unknown { exe_path: PathBuf },
}

/// Where Cargo pulled the binary from. Defaults to `Unknown` until the
/// dispatcher resolves the actual source for the upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CargoSource {
    /// Source not yet resolved — dispatcher will check env + flags.
    #[default]
    Unresolved,
    /// User passed an explicit source path (`--source <path>` or
    /// `MAKAKOO_SOURCE_PATH` env var).
    LocalPath(PathBuf),
    /// Default fallback — pull fresh source from the public repo.
    Git(String),
}

/// Detect the install method by inspecting the running binary's path.
///
/// Resolves symlinks via `canonicalize`. Test seam: callers can pass
/// in an explicit `exe_path` + `home_dir` to bypass real probes.
pub fn detect_install_method() -> InstallMethod {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return InstallMethod::Unknown {
            exe_path: PathBuf::from("<unknown>"),
        },
    };
    let canonical = std::fs::canonicalize(&exe).unwrap_or(exe);
    let home = dirs::home_dir().unwrap_or_default();
    classify(&canonical, &home)
}

/// Pure classifier. Tests pass synthetic paths.
pub fn classify(exe: &Path, home: &Path) -> InstallMethod {
    // Reject dev builds — running from `target/debug/` or `target/release/`
    // means the user hasn't installed; they should use `cargo install --path`.
    let s = exe.to_string_lossy();
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        return InstallMethod::Unknown {
            exe_path: exe.to_path_buf(),
        };
    }

    // Cargo: $HOME/.cargo/bin/
    let cargo_bin = home.join(".cargo").join("bin");
    if exe.starts_with(&cargo_bin) {
        return InstallMethod::Cargo {
            source: CargoSource::Unresolved,
        };
    }

    // Homebrew prefixes (Apple Silicon + Intel + Linuxbrew).
    //
    // Match either the user-visible `<prefix>/bin/` symlink or the actual
    // `<prefix>/Cellar/...` package store — `canonicalize()` resolves the
    // symlink to the Cellar path, so a real brew install reaches us as
    // `/usr/local/Cellar/makakoo/0.1.3/bin/makakoo` rather than
    // `/usr/local/bin/makakoo`.
    for brew_prefix in [
        Path::new("/opt/homebrew"),
        Path::new("/usr/local"),
        Path::new("/home/linuxbrew/.linuxbrew"),
    ] {
        if exe.starts_with(brew_prefix.join("bin"))
            || exe.starts_with(brew_prefix.join("Cellar"))
        {
            return InstallMethod::Homebrew {
                prefix: brew_prefix.to_path_buf(),
            };
        }
    }

    // Curl-pipe: $MAKAKOO_PREFIX/bin/ (default $HOME/.local/bin/).
    let curl_prefix = std::env::var("MAKAKOO_PREFIX")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local"));
    if exe.starts_with(curl_prefix.join("bin")) {
        return InstallMethod::CurlPipe {
            prefix: curl_prefix,
        };
    }

    InstallMethod::Unknown {
        exe_path: exe.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn classifies_cargo_install() {
        let result = classify(
            &p("/Users/sebastian/.cargo/bin/makakoo"),
            &p("/Users/sebastian"),
        );
        assert!(matches!(
            result,
            InstallMethod::Cargo {
                source: CargoSource::Unresolved
            }
        ));
    }

    #[test]
    fn classifies_homebrew_apple_silicon() {
        let result = classify(&p("/opt/homebrew/bin/makakoo"), &p("/Users/sebastian"));
        match result {
            InstallMethod::Homebrew { prefix } => {
                assert_eq!(prefix, p("/opt/homebrew"));
            }
            other => panic!("expected Homebrew, got {other:?}"),
        }
    }

    #[test]
    fn classifies_homebrew_intel() {
        let result = classify(&p("/usr/local/bin/makakoo"), &p("/Users/sebastian"));
        assert!(matches!(result, InstallMethod::Homebrew { .. }));
    }

    #[test]
    fn classifies_homebrew_linuxbrew() {
        let result = classify(
            &p("/home/linuxbrew/.linuxbrew/bin/makakoo"),
            &p("/home/linuxbrew"),
        );
        assert!(matches!(result, InstallMethod::Homebrew { .. }));
    }

    #[test]
    fn classifies_homebrew_canonicalized_cellar_path_intel() {
        // What `canonicalize()` actually produces on real Intel brew installs:
        // the symlink `/usr/local/bin/makakoo` resolves to the Cellar version
        // dir. Earlier versions of the detector only matched `<prefix>/bin/`
        // and mis-classified this as Unknown.
        let result = classify(
            &p("/usr/local/Cellar/makakoo/0.1.3/bin/makakoo"),
            &p("/Users/sebastian"),
        );
        match result {
            InstallMethod::Homebrew { prefix } => {
                assert_eq!(prefix, p("/usr/local"));
            }
            other => panic!("expected Homebrew, got {other:?}"),
        }
    }

    #[test]
    fn classifies_homebrew_canonicalized_cellar_path_apple_silicon() {
        let result = classify(
            &p("/opt/homebrew/Cellar/makakoo/0.1.3/bin/makakoo"),
            &p("/Users/sebastian"),
        );
        match result {
            InstallMethod::Homebrew { prefix } => {
                assert_eq!(prefix, p("/opt/homebrew"));
            }
            other => panic!("expected Homebrew, got {other:?}"),
        }
    }

    #[test]
    fn classifies_curl_pipe_default_prefix() {
        // Clear the env override before the classify call.
        std::env::remove_var("MAKAKOO_PREFIX");
        let result = classify(
            &p("/Users/sebastian/.local/bin/makakoo"),
            &p("/Users/sebastian"),
        );
        match result {
            InstallMethod::CurlPipe { prefix } => {
                assert_eq!(prefix, p("/Users/sebastian/.local"));
            }
            other => panic!("expected CurlPipe, got {other:?}"),
        }
    }

    #[test]
    fn rejects_dev_build_target_debug() {
        let result = classify(
            &p("/Users/sebastian/makakoo-os/target/debug/makakoo"),
            &p("/Users/sebastian"),
        );
        assert!(matches!(result, InstallMethod::Unknown { .. }));
    }

    #[test]
    fn rejects_dev_build_target_release() {
        let result = classify(
            &p("/Users/sebastian/makakoo-os/target/release/makakoo"),
            &p("/Users/sebastian"),
        );
        assert!(matches!(result, InstallMethod::Unknown { .. }));
    }

    #[test]
    fn unknown_for_random_path() {
        let result = classify(
            &p("/opt/something/weird/makakoo"),
            &p("/Users/sebastian"),
        );
        assert!(matches!(result, InstallMethod::Unknown { .. }));
    }

    #[test]
    fn unknown_when_only_root_path_matches_nothing() {
        let result = classify(&p("/makakoo"), &p("/Users/sebastian"));
        assert!(matches!(result, InstallMethod::Unknown { .. }));
    }

    #[test]
    fn cargo_takes_priority_over_curl_pipe() {
        // A user who has both ~/.cargo/bin/makakoo and ~/.local/bin/makakoo
        // would only have one of them on PATH first, but if the binary IS
        // in ~/.cargo/bin, that's the canonical install we should upgrade.
        let result = classify(
            &p("/Users/sebastian/.cargo/bin/makakoo"),
            &p("/Users/sebastian"),
        );
        assert!(matches!(
            result,
            InstallMethod::Cargo {
                source: CargoSource::Unresolved
            }
        ));
    }
}
