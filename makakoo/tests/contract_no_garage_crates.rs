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
fn agpl_pin_documented_in_garage_store_install_sh() {
    // Defensive: if someone bumps Garage but forgets to re-pin the
    // formula tarball SHA in install.sh, we want a loud failure.
    // The pin is the gate — without it, the brew formula floats and
    // we can't audit what AGPL source is actually being executed.
    let install_sh = workspace_root()
        .join("plugins-core/garage-store/bin/install.sh");
    let body = fs::read_to_string(&install_sh)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", install_sh.display()));
    let pin = "b83a981677676b35400bbbaf20974c396f32da31c7c7630ce55fc3e62c0e2e01";
    assert!(
        body.contains(pin),
        "\nAGPL gate: garage-store install.sh missing the pinned formula SHA.\n\
         \n\
         Expected to find: {pin}\n\
         in: {}\n\
         \n\
         When bumping Garage, refresh both:\n\
           - the brew formula commit SHA (tracks formula evolution)\n\
           - the upstream tarball sha256 (tracks the actual AGPL source)\n",
        install_sh.display()
    );
}
