//! Contract test: no Garage Rust crate may be linked into the Makakoo
//! binary. AGPLv3 distribution gate from Locked Decision 7 of
//! `MAKAKOO-OS-V0.7-S3-ENDPOINT`.
//!
//! Garage is sourced via Homebrew which builds from the AGPL upstream
//! tarball (`git.deuxfleurs.fr/Deuxfleurs/garage v2.3.0`,
//! sha256 b83a981677676b35400bbbaf20974c396f32da31c7c7630ce55fc3e62c0e2e01).
//! The `garage` binary is invoked as a **subprocess** by
//! `plugins-core/garage-store/`, never linked.
//!
//! Subprocess invocation does not trigger AGPL distribution
//! obligations on the Makakoo binary. Linking would. This test
//! catches the linking case at CI time so a future PR cannot
//! accidentally pull in `garage*` Rust crates as a Cargo dependency.

use std::fs;
use std::path::PathBuf;

/// Crate-name substrings that flag AGPL Garage code in Cargo.lock.
/// `garage` matches the top-level crate; `garage_*` / `garage-*` match
/// the workspace sub-crates upstream actually publishes.
const FORBIDDEN_CRATE_PREFIXES: &[&str] = &["garage", "garage_", "garage-"];

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR points at makakoo/. The workspace Cargo.lock
    // lives one directory up.
    p.pop();
    p
}

#[test]
fn cargo_lock_has_no_garage_crate() {
    let lock_path = workspace_root().join("Cargo.lock");
    let lock = fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lock_path.display()));

    let mut hits: Vec<String> = Vec::new();
    for (lineno, line) in lock.lines().enumerate() {
        let trimmed = line.trim();
        // Cargo.lock package entries look like: name = "<crate>"
        // We match on the `name = "..."` form to avoid false positives
        // on `dependencies = [...]` lines that mention crate names.
        let Some(rest) = trimmed.strip_prefix("name = \"") else {
            continue;
        };
        let Some(end) = rest.find('"') else { continue };
        let crate_name = &rest[..end];
        for forbidden in FORBIDDEN_CRATE_PREFIXES {
            // Exact match for "garage" (the top-level crate); prefix
            // match for the `garage_<sub>` / `garage-<sub>` workspace
            // crates. We deliberately do NOT match arbitrary substring
            // hits (a crate named `mygaragepkg` would not be Garage
            // upstream code).
            let is_forbidden = if *forbidden == "garage" {
                crate_name == "garage"
            } else {
                crate_name.starts_with(forbidden)
            };
            if is_forbidden {
                hits.push(format!(
                    "  Cargo.lock:{} — `name = \"{crate_name}\"`",
                    lineno + 1
                ));
                break;
            }
        }
    }

    if !hits.is_empty() {
        panic!(
            "\n❌ AGPL gate violation: Garage Rust crate(s) found in Cargo.lock.\n\
             \n\
             Linking AGPL code into the Makakoo binary triggers source-distribution\n\
             obligations on the entire combined work. Garage must be invoked as a\n\
             subprocess only. Sourced via Homebrew, never as a Cargo dependency.\n\
             \n\
             Locked Decision 7 of MAKAKOO-OS-V0.7-S3-ENDPOINT.\n\
             \n\
             Offending entries:\n{}\n",
            hits.join("\n")
        );
    }
}

#[test]
fn garage_store_install_sh_defers_to_garagetytus() {
    // Pre-GARAGETYTUS-V0.1, this file embedded a pinned brew-formula
    // SHA so the AGPL audit could verify which Garage source was being
    // pulled. As of Phase D the install hook is a deferral shim — the
    // daemon lifecycle and Garage acquisition both moved to the
    // standalone `garagetytus` binary, where the AGPL pinning lives now.
    //
    // This test now enforces the deferral pattern: install.sh must NOT
    // download the brew formula directly. If you find yourself wanting
    // to add the SHA pin back here, the lifecycle is regressing —
    // re-read garage-store's Phase D adapter shim comments first.
    let install_sh = workspace_root()
        .join("plugins-core/garage-store/bin/install.sh");
    let body = fs::read_to_string(&install_sh)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", install_sh.display()));

    // Either the file defers to `garagetytus` on PATH, or it tells the
    // operator how to install garagetytus. Both shapes are valid for
    // the deferral contract.
    let defers = body.contains("garagetytus") && body.contains("install");
    assert!(
        defers,
        "\nGarage AGPL gate (post-Phase-D): garage-store install.sh must defer\n\
         to the standalone garagetytus binary, not embed Garage acquisition.\n\
         \n\
         Expected substring: \"garagetytus\" + \"install\"\n\
         in: {}\n\
         \n\
         If you're re-adding a brew SHA pin here, audit ownership has\n\
         regressed. Pin SHAs live in the garagetytus repo now.\n",
        install_sh.display()
    );

    // Belt-and-braces: the bare `brew install` of a SHA-tarball pattern
    // (the regressed shape) must not appear here.
    let regressed = body.contains("--build-from-source") || body.contains("sha256:");
    assert!(
        !regressed,
        "\ngarage-store install.sh appears to embed brew-source-build flags\n\
         again. Move the AGPL pin into garagetytus and keep this file as\n\
         a thin deferral shim.\n"
    );
}
