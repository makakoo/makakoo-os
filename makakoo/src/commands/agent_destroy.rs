//! `makakoo agent destroy <slot>` — interactive teardown CLI.
//!
//! Wraps `agents::destroy::destroy_slot` with:
//!   * confirmation prompt (skipped via --yes)
//!   * supervisor stop (best-effort) before archive
//!   * `--revoke-secrets` follow-up that removes detected refs from
//!     the keyring after a successful destroy

use std::time::{SystemTime, UNIX_EPOCH};

use makakoo_core::agents::destroy::{
    destroy_slot, render_restore_one_liner, DestroyError, PROTECTED_SLOT,
};

use crate::secrets::SecretsStore;

use crate::context::CliContext;
use crate::output;

/// Args bundle so the dispatch site doesn't keep growing.
pub struct DestroyArgs {
    pub slot: String,
    pub yes: bool,
    pub revoke_secrets: bool,
    /// `--keep-secrets` is the default behavior; we accept the flag
    /// for explicit clarity but it's a no-op.
    pub keep_secrets: bool,
    pub really_destroy_harveychat: bool,
}

pub fn run(ctx: &CliContext, args: DestroyArgs) -> anyhow::Result<i32> {
    let DestroyArgs {
        slot,
        yes,
        revoke_secrets,
        keep_secrets,
        really_destroy_harveychat,
    } = args;
    let _ = keep_secrets; // explicit no-op (default already preserves)
    let home = ctx.home();

    // Round-2 fix: protected-slot guard runs BEFORE prompt + stop so
    // refusing to destroy `harveychat` has zero side effects (the
    // supervisor is not stopped, no prompt is shown, no archive is
    // created).
    if slot == PROTECTED_SLOT && !really_destroy_harveychat {
        output::print_error(DestroyError::HarveychatProtected.to_string());
        return Ok(64);
    }

    // Confirm.
    if !yes {
        let mut prompt = format!(
            "About to destroy slot '{slot}'. The TOML and data dir will be \
             moved to $MAKAKOO_HOME/archive/agents/{slot}-<ts>/. \
             Continue? [y/N] "
        );
        if revoke_secrets {
            prompt.push_str(
                "(--revoke-secrets is set: detected secret refs will be \
                 removed from the keyring after archive succeeds.) ",
            );
        }
        if !prompt_confirm(&prompt) {
            output::print_warn("destroy cancelled");
            return Ok(2);
        }
    }

    // Stop (best-effort but loud — a wedged supervisor warrants a
    // visible warn rather than silent continuation).
    if crate::commands::agent_lifecycle::is_slot(home, &slot) {
        match crate::commands::agent_lifecycle::stop_slot(ctx, &slot) {
            Ok(0) => {}
            Ok(rc) => output::print_warn(format!(
                "stop returned exit {rc} — supervisor may already be down; continuing"
            )),
            Err(e) => output::print_warn(format!(
                "stop failed ({e}) — supervisor may already be down; continuing"
            )),
        }
    }

    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let outcome = match destroy_slot(home, &slot, really_destroy_harveychat, unix_ts) {
        Ok(o) => o,
        Err(DestroyError::HarveychatProtected) => {
            output::print_error(DestroyError::HarveychatProtected.to_string());
            return Ok(64);
        }
        Err(DestroyError::SlotNotFound { slot_id, path }) => {
            output::print_error(format!(
                "slot '{slot_id}' not found at {}",
                path.display()
            ));
            return Ok(1);
        }
        Err(DestroyError::ArchiveExists { path }) => {
            output::print_error(format!(
                "archive dir already exists at {}: refusing to overwrite",
                path.display()
            ));
            return Ok(1);
        }
    };

    println!("destroyed slot '{}':", outcome.slot_id);
    println!("  archive: {}", outcome.archive_dir.display());
    if let Some(d) = &outcome.archived_data_dir {
        println!("  data archived: {}", d.display());
    }

    if !outcome.detected_secrets.is_empty() {
        println!(
            "\ndetected {} secret reference(s):",
            outcome.detected_secrets.len()
        );
        for s in &outcome.detected_secrets {
            println!("  - {s}");
        }
        println!(
            "  (note: secrets nested under [transport.config] sub-tables \
             or referenced via env-var interpolation are NOT detected)"
        );

        // Decide revoke (locked Q3 truth table — see decide_revoke).
        let prompt_response = if !revoke_secrets && !yes {
            Some(prompt_confirm(&format!(
                "\nRevoke these {} detected secrets too? [y/N] ",
                outcome.detected_secrets.len()
            )))
        } else {
            None
        };
        let do_revoke = decide_revoke(yes, revoke_secrets, prompt_response);

        if do_revoke {
            println!("\nrevoking secrets from keyring:");
            let mut revoked = 0;
            let mut failed = 0;
            for s in &outcome.detected_secrets {
                match SecretsStore::delete(s) {
                    Ok(()) => {
                        println!("  ✓ {s}");
                        revoked += 1;
                    }
                    Err(e) => {
                        output::print_warn(format!("  ✗ {s}: {e}"));
                        failed += 1;
                    }
                }
            }
            println!("\n{revoked} secret(s) revoked, {failed} failed");
        } else {
            println!(
                "\n(secrets PRESERVED — re-run with --revoke-secrets to delete \
                 them from the keyring.)"
            );
        }
    }

    println!("\n{}", render_restore_one_liner(&outcome, home));
    Ok(0)
}

fn prompt_confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Pure decision function for whether destroy should revoke detected
/// secrets. Extracted so the three-way logic is unit-testable
/// without stdin / stdout capture. Mirrors locked Q3:
///
/// | --revoke-secrets | --yes | prompt   | revoke?|
/// |       true       | any   | n/a      |   yes  |
/// |       false      | true  | n/a      |   NO   |
/// |       false      | false | yes      |   yes  |
/// |       false      | false | no/none  |   NO   |
pub fn decide_revoke(yes: bool, revoke_flag: bool, prompt_response: Option<bool>) -> bool {
    if revoke_flag {
        true
    } else if !yes {
        prompt_response.unwrap_or(false)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::agents::destroy::slot_data_dir;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn ctx_for(home: &Path) -> CliContext {
        CliContext::for_home(home.to_path_buf())
    }

    fn write_slot(home: &Path, slot_id: &str, body: &str) {
        let cfg = home.join("config/agents");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join(format!("{slot_id}.toml")), body).unwrap();
    }

    fn args(slot: &str, yes: bool) -> DestroyArgs {
        DestroyArgs {
            slot: slot.into(),
            yes,
            revoke_secrets: false,
            keep_secrets: false,
            really_destroy_harveychat: false,
        }
    }

    #[test]
    fn destroy_yes_succeeds_without_prompt() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        let rc = run(&ctx, args("secretary", true)).unwrap();
        assert_eq!(rc, 0);
        assert!(!tmp
            .path()
            .join("config/agents/secretary.toml")
            .exists());
    }

    #[test]
    fn destroy_missing_slot_returns_1() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        let rc = run(&ctx, args("ghost", true)).unwrap();
        assert_eq!(rc, 1);
    }

    #[test]
    fn destroy_harveychat_without_flag_returns_64() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "harveychat", "slot_id = \"harveychat\"\n");
        let rc = run(&ctx, args("harveychat", true)).unwrap();
        assert_eq!(rc, 64, "harveychat refusal must use exit 64 (EX_USAGE)");
        // TOML must still exist.
        assert!(tmp
            .path()
            .join("config/agents/harveychat.toml")
            .exists());
    }

    #[test]
    fn destroy_harveychat_with_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "harveychat", "slot_id = \"harveychat\"\n");
        let mut a = args("harveychat", true);
        a.really_destroy_harveychat = true;
        let rc = run(&ctx, a).unwrap();
        assert_eq!(rc, 0);
    }

    // Note: archive-collision rejection is tested deterministically
    // in `agents::destroy::tests::destroy_refuses_overwriting_existing_archive`
    // which controls `unix_ts` directly. The CLI uses the wall clock,
    // so a CLI-level collision test would race the wall second and
    // prove nothing beyond what the core test covers.

    #[test]
    fn destroy_keep_secrets_is_a_noop() {
        // Default behavior preserves secrets; the flag is for clarity.
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        let mut a = args("secretary", true);
        a.keep_secrets = true;
        let rc = run(&ctx, a).unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn destroy_with_data_dir_archives_data() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        let data = slot_data_dir(tmp.path(), "secretary");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("conv.db"), "fake").unwrap();
        let rc = run(&ctx, args("secretary", true)).unwrap();
        assert_eq!(rc, 0);
        assert!(!data.exists(), "data dir must be moved");
    }

    // ── Round-2 (codex review) regressions ────────────────────────

    #[test]
    fn decide_revoke_explicit_flag_always_wins() {
        assert!(decide_revoke(true, true, None));
        assert!(decide_revoke(true, true, Some(false)));
        assert!(decide_revoke(false, true, Some(false)));
    }

    #[test]
    fn decide_revoke_yes_without_flag_preserves_secrets() {
        // Codex-flagged invariant: `--yes` ALONE must NOT auto-revoke.
        assert!(!decide_revoke(true, false, None));
        assert!(
            !decide_revoke(true, false, Some(true)),
            "even a stale prompt response must not revoke when --yes is set without --revoke-secrets"
        );
    }

    #[test]
    fn decide_revoke_interactive_yes_response_revokes() {
        assert!(decide_revoke(false, false, Some(true)));
    }

    #[test]
    fn decide_revoke_interactive_no_or_missing_response_preserves() {
        assert!(!decide_revoke(false, false, Some(false)));
        assert!(!decide_revoke(false, false, None));
    }

    #[test]
    fn destroy_harveychat_without_flag_has_no_side_effects() {
        // Codex round-2 regression: protected-slot refusal must
        // happen BEFORE supervisor stop / archive / any prompt.
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "harveychat", "slot_id = \"harveychat\"\n");
        let data = slot_data_dir(tmp.path(), "harveychat");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("conv.db"), "important data").unwrap();

        let rc = run(&ctx, args("harveychat", true)).unwrap();
        assert_eq!(rc, 64);
        assert!(tmp.path().join("config/agents/harveychat.toml").exists());
        assert!(data.exists(), "data dir must be untouched");
        assert_eq!(
            fs::read_to_string(data.join("conv.db")).unwrap(),
            "important data"
        );
        let archive_root = tmp.path().join("archive/agents");
        if archive_root.exists() {
            let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().collect();
            assert!(entries.is_empty(), "no archive entries should exist");
        }
    }

    #[test]
    fn destroy_archives_empty_data_dir_when_source_absent() {
        // Locked Q3 archive shape: <slot>.toml + data/ — even when
        // the source had no data dir.
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(tmp.path());
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        let rc = run(&ctx, args("secretary", true)).unwrap();
        assert_eq!(rc, 0);
        let archive_root = tmp.path().join("archive/agents");
        let archived = fs::read_dir(&archive_root)
            .unwrap()
            .next()
            .expect("at least one archive entry")
            .unwrap();
        assert!(archived.path().join("data").exists());
        assert!(archived.path().join("secretary.toml").exists());
    }
}
