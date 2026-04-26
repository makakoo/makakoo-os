//! macOS launchd integration — plist generator + bootstrap with
//! Files & Folders consent error detection.
//!
//! Locked design (Phase 0 Q1):
//!
//! 1. `LaunchAgentPlist::from_slot(slot_id, makakoo_bin)` returns the
//!    XML for `~/Library/LaunchAgents/com.makakoo.agent.<slot>.plist`.
//!
//! 2. `LaunchctlInstaller::bootstrap(plist_path)` runs `launchctl
//!    bootstrap gui/<uid>` and translates the well-known "operation
//!    not permitted" exit (code 5) into a structured error so the CLI
//!    can print the Files & Folders consent remediation hint.
//!
//! All file / process I/O is gated behind a thin `LaunchctlExec`
//! trait so tests can substitute a fake.

use std::path::{Path, PathBuf};

use crate::agents::slot::validate_slot_id;
use crate::error::{MakakooError, Result};

/// Reverse-DNS prefix for the launchctl service label. Matches the
/// plist filename: `com.makakoo.agent.<slot>.plist`.
pub const LABEL_PREFIX: &str = "com.makakoo.agent.";

/// Locked launchctl exit code that means "Files & Folders consent
/// not granted" — surfaces from launchctl when SIP / TCC denies the
/// caller permission to write into `~/Library/LaunchAgents`.
pub const EX_NOT_PERMITTED: i32 = 5;

/// Generated LaunchAgent plist body + the path it should land at.
#[derive(Debug, Clone)]
pub struct LaunchAgentPlist {
    pub label: String,
    pub plist_xml: String,
    pub plist_path: PathBuf,
}

impl LaunchAgentPlist {
    /// Build a plist for the given slot.
    ///
    /// `makakoo_bin` is the absolute path to the `makakoo` binary.
    /// `os_home` is the OS user home directory ($HOME) — the plist
    /// MUST land under `$HOME/Library/LaunchAgents/` (launchd
    /// requirement, not configurable).
    /// `makakoo_home` is the Makakoo install root — used for log
    /// file paths under `$MAKAKOO_HOME/data/log/`.
    pub fn from_slot(
        slot_id: &str,
        makakoo_bin: &Path,
        os_home: &Path,
        makakoo_home: &Path,
    ) -> Result<Self> {
        validate_slot_id(slot_id).map_err(|e| {
            MakakooError::Internal(format!("invalid slot id '{slot_id}': {e}"))
        })?;
        let label = format!("{LABEL_PREFIX}{slot_id}");
        let plist_path = os_home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        let xml = render_plist_xml(&label, makakoo_bin, slot_id, makakoo_home);
        Ok(Self {
            label,
            plist_xml: xml,
            plist_path,
        })
    }

    /// Persist the plist body to `plist_path`. Creates the
    /// `LaunchAgents` directory if missing. Returns the path written.
    pub fn write(&self) -> Result<&Path> {
        if let Some(parent) = self.plist_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MakakooError::Internal(format!("create LaunchAgents dir: {e}"))
            })?;
        }
        std::fs::write(&self.plist_path, &self.plist_xml).map_err(|e| {
            MakakooError::Internal(format!("write {}: {e}", self.plist_path.display()))
        })?;
        Ok(&self.plist_path)
    }
}

/// Result of a launchctl call. Pulled out as a separate trait so
/// tests can stub it.
pub struct LaunchctlOutput {
    pub exit_code: i32,
    pub stderr: String,
}

pub trait LaunchctlExec: Send + Sync {
    fn bootstrap(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput>;
    fn bootout(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput>;
}

/// Real launchctl driver. Shells out to `/bin/launchctl`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealLaunchctl;

impl LaunchctlExec for RealLaunchctl {
    fn bootstrap(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput> {
        run_launchctl(&["bootstrap", &format!("gui/{uid}"), &plist_path.to_string_lossy()])
    }
    fn bootout(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput> {
        run_launchctl(&["bootout", &format!("gui/{uid}"), &plist_path.to_string_lossy()])
    }
}

fn run_launchctl(args: &[&str]) -> Result<LaunchctlOutput> {
    let out = std::process::Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|e| MakakooError::Internal(format!("invoke launchctl: {e}")))?;
    Ok(LaunchctlOutput {
        exit_code: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Errors specific to the launchctl bootstrap path.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// launchctl returned exit 5 — "operation not permitted". The
    /// user must grant Files & Folders consent in System Settings
    /// → Privacy & Security.
    #[error(
        "launchctl exit 5 — Files & Folders consent denied. \
         Open System Settings → Privacy & Security → Files & Folders \
         and grant the terminal/IDE you ran 'makakoo' from access to \
         your home folder, then re-run 'makakoo agent start <slot>'."
    )]
    FilesAndFoldersConsent,

    /// Already loaded — bootstrap fails when the same label is
    /// active. Treated as success at the higher CLI layer.
    #[error("plist already loaded ({label})")]
    AlreadyLoaded { label: String },

    /// Anything else launchctl complained about.
    #[error("launchctl bootstrap failed (exit {exit_code}): {stderr}")]
    Other { exit_code: i32, stderr: String },
}

impl BootstrapError {
    /// Translate a `LaunchctlOutput` from `bootstrap` into a
    /// structured `BootstrapError`. The locked translation set:
    ///
    /// * exit 0                                 → Ok
    /// * exit 5                                 → FilesAndFoldersConsent
    /// * stderr contains "Service is disabled"  → also consent (sips block)
    /// * stderr contains "already loaded"       → AlreadyLoaded
    /// * else                                   → Other
    pub fn from_output(out: LaunchctlOutput, label: &str) -> Result<()> {
        if out.exit_code == 0 {
            return Ok(());
        }
        if out.exit_code == EX_NOT_PERMITTED {
            return Err(MakakooError::Internal(
                BootstrapError::FilesAndFoldersConsent.to_string(),
            ));
        }
        if out.stderr.contains("Service is disabled") {
            return Err(MakakooError::Internal(
                BootstrapError::FilesAndFoldersConsent.to_string(),
            ));
        }
        if out.stderr.to_lowercase().contains("already loaded")
            || out.stderr.to_lowercase().contains("already bootstrapped")
        {
            return Err(MakakooError::Internal(
                BootstrapError::AlreadyLoaded {
                    label: label.to_string(),
                }
                .to_string(),
            ));
        }
        Err(MakakooError::Internal(
            BootstrapError::Other {
                exit_code: out.exit_code,
                stderr: out.stderr,
            }
            .to_string(),
        ))
    }
}

/// Render the LaunchAgent plist XML. Locked schema:
///
/// * `KeepAlive=true`   — launchd auto-restarts on crash (the
///                        user-space restart budget short-circuits
///                        before this kicks in for normal cases).
/// * `RunAtLoad=true`   — start immediately on bootstrap.
/// * `ProcessType=Interactive` — gives us reasonable scheduling
///                        priority for foreground UX (Telegram
///                        long-poll, Slack WS).
/// * `ThrottleInterval=10` — minimum interval between launchd
///                        respawns; keeps a wedged supervisor from
///                        burning the CPU.
/// XML-escape a string for safe embedding inside a plist `<string>`.
/// Only `&`, `<`, `>`, `"`, `'` need escaping in PLIST element bodies.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn render_plist_xml(
    label: &str,
    makakoo_bin: &Path,
    slot_id: &str,
    makakoo_home: &Path,
) -> String {
    let bin = xml_escape(&makakoo_bin.to_string_lossy());
    let label_e = xml_escape(label);
    let slot_e = xml_escape(slot_id);
    let stdout = xml_escape(
        &makakoo_home
            .join(format!("data/log/agent-{slot_id}.out.log"))
            .to_string_lossy(),
    );
    let stderr = xml_escape(
        &makakoo_home
            .join(format!("data/log/agent-{slot_id}.err.log"))
            .to_string_lossy(),
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label_e}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>agent</string>
        <string>_supervisor</string>
        <string>--slot</string>
        <string>{slot_e}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>MAKAKOO_AGENT_SLOT</key>
        <string>{slot_e}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#
    )
}

/// Effective uid for `gui/<uid>` bootstrap target.
pub fn current_uid() -> u32 {
    // SAFETY: getuid is signal-safe and always succeeds.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn plist_path_is_under_user_library_launchagents() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let p = LaunchAgentPlist::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        assert_eq!(p.label, "com.makakoo.agent.secretary");
        assert!(
            p.plist_path
                .ends_with("Library/LaunchAgents/com.makakoo.agent.secretary.plist"),
            "plist path: {}",
            p.plist_path.display()
        );
    }

    #[test]
    fn plist_xml_embeds_slot_id_supervisor_subcommand_and_logs() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let p = LaunchAgentPlist::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        assert!(p.plist_xml.contains("<string>com.makakoo.agent.secretary</string>"));
        assert!(p.plist_xml.contains("<string>/usr/local/bin/makakoo</string>"));
        assert!(p.plist_xml.contains("<string>_supervisor</string>"));
        assert!(p.plist_xml.contains("<string>secretary</string>"));
        assert!(p.plist_xml.contains("agent-secretary.out.log"));
        assert!(p.plist_xml.contains("agent-secretary.err.log"));
        assert!(p.plist_xml.contains("<key>KeepAlive</key>"));
        assert!(p.plist_xml.contains("<key>ThrottleInterval</key>"));
    }

    #[test]
    fn plist_rejects_invalid_slot_id() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let err =
            LaunchAgentPlist::from_slot("Bad Slot!", &bin, home.path(), home.path()).err();
        assert!(err.is_some(), "expected validation error");
    }

    #[test]
    fn plist_path_uses_os_home_log_paths_use_makakoo_home() {
        let os_home = TempDir::new().unwrap();
        let makakoo_home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let p =
            LaunchAgentPlist::from_slot("secretary", &bin, os_home.path(), makakoo_home.path())
                .unwrap();
        assert!(
            p.plist_path.starts_with(os_home.path()),
            "plist must land under OS home, not Makakoo home. Got: {}",
            p.plist_path.display()
        );
        assert!(
            !p.plist_path.starts_with(makakoo_home.path()),
            "plist must NOT land under Makakoo home"
        );
        let mh_str = makakoo_home.path().to_string_lossy().into_owned();
        assert!(
            p.plist_xml
                .contains(&format!("{mh_str}/data/log/agent-secretary.out.log")),
            "stdout log must use Makakoo-home path"
        );
    }

    #[test]
    fn plist_xml_escapes_special_chars_in_paths() {
        let os_home = TempDir::new().unwrap();
        let makakoo_home = TempDir::new().unwrap();
        let bin = PathBuf::from("/opt/My Apps & Tools/<makakoo>");
        let p =
            LaunchAgentPlist::from_slot("secretary", &bin, os_home.path(), makakoo_home.path())
                .unwrap();
        assert!(
            !p.plist_xml.contains("/<makakoo>"),
            "raw '<' in path must be escaped"
        );
        assert!(p.plist_xml.contains("&amp;"), "raw '&' must be escaped");
        assert!(p.plist_xml.contains("&lt;makakoo&gt;"));
    }

    #[test]
    fn plist_write_creates_launchagents_dir_and_file() {
        let home = TempDir::new().unwrap();
        let bin = PathBuf::from("/usr/local/bin/makakoo");
        let p = LaunchAgentPlist::from_slot("secretary", &bin, home.path(), home.path()).unwrap();
        let path = p.write().unwrap();
        assert!(path.exists(), "plist file must be created");
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("com.makakoo.agent.secretary"));
    }

    #[test]
    fn bootstrap_error_consent_from_exit_5() {
        let out = LaunchctlOutput {
            exit_code: 5,
            stderr: "Operation not permitted".into(),
        };
        let err = BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Files & Folders"), "got: {msg}");
        assert!(msg.contains("Privacy & Security"));
    }

    #[test]
    fn bootstrap_error_consent_from_disabled_text() {
        let out = LaunchctlOutput {
            exit_code: 1,
            stderr: "Service is disabled".into(),
        };
        let err = BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap_err();
        assert!(err.to_string().contains("Files & Folders"));
    }

    #[test]
    fn bootstrap_error_already_loaded_is_distinct() {
        let out = LaunchctlOutput {
            exit_code: 17,
            stderr: "Service already loaded".into(),
        };
        let err = BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap_err();
        assert!(err.to_string().contains("already loaded"));
    }

    #[test]
    fn bootstrap_error_other_preserves_exit_and_stderr() {
        let out = LaunchctlOutput {
            exit_code: 42,
            stderr: "weird thing happened".into(),
        };
        let err = BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("42"));
        assert!(msg.contains("weird thing"));
    }

    #[test]
    fn bootstrap_success_returns_ok() {
        let out = LaunchctlOutput {
            exit_code: 0,
            stderr: String::new(),
        };
        BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap();
    }

    /// MockLaunchctl — a fake exec that records what was called and
    /// returns scripted outputs. Lets integration tests assert
    /// bootstrap/bootout sequencing without running real launchctl.
    pub struct MockLaunchctl {
        pub calls: std::sync::Mutex<Vec<(String, u32, PathBuf)>>,
        pub bootstrap_output: std::sync::Mutex<LaunchctlOutput>,
        pub bootout_output: std::sync::Mutex<LaunchctlOutput>,
    }

    impl MockLaunchctl {
        pub fn ok() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                bootstrap_output: std::sync::Mutex::new(LaunchctlOutput {
                    exit_code: 0,
                    stderr: String::new(),
                }),
                bootout_output: std::sync::Mutex::new(LaunchctlOutput {
                    exit_code: 0,
                    stderr: String::new(),
                }),
            }
        }
    }

    impl LaunchctlExec for MockLaunchctl {
        fn bootstrap(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput> {
            self.calls.lock().unwrap().push((
                "bootstrap".into(),
                uid,
                plist_path.to_path_buf(),
            ));
            let g = self.bootstrap_output.lock().unwrap();
            Ok(LaunchctlOutput {
                exit_code: g.exit_code,
                stderr: g.stderr.clone(),
            })
        }
        fn bootout(&self, uid: u32, plist_path: &Path) -> Result<LaunchctlOutput> {
            self.calls.lock().unwrap().push((
                "bootout".into(),
                uid,
                plist_path.to_path_buf(),
            ));
            let g = self.bootout_output.lock().unwrap();
            Ok(LaunchctlOutput {
                exit_code: g.exit_code,
                stderr: g.stderr.clone(),
            })
        }
    }

    #[test]
    fn mock_launchctl_records_bootstrap_calls() {
        let mock = MockLaunchctl::ok();
        let path = PathBuf::from("/x.plist");
        mock.bootstrap(501, &path).unwrap();
        mock.bootout(501, &path).unwrap();
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "bootstrap");
        assert_eq!(calls[1].0, "bootout");
        assert_eq!(calls[0].1, 501);
        assert_eq!(calls[0].2, path);
    }

    #[test]
    fn mock_launchctl_returns_consent_when_scripted() {
        let mock = MockLaunchctl::ok();
        *mock.bootstrap_output.lock().unwrap() = LaunchctlOutput {
            exit_code: 5,
            stderr: "Operation not permitted".into(),
        };
        let out = mock.bootstrap(501, &PathBuf::from("/x.plist")).unwrap();
        let err = BootstrapError::from_output(out, "com.makakoo.agent.x").unwrap_err();
        assert!(err.to_string().contains("Files & Folders"));
    }
}
