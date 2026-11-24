//! Cross-platform regression tests for sibling workspace binary resolution.
//!
//! Deliberately NOT `cfg(unix)` so Windows CI exercises the exact same
//! `EXE_SUFFIX`, profile-directory, ancestor-isolation, and CI fallback guards.

mod common;

use std::ffi::OsString;
use std::path::Path;

use tempfile::TempDir;

#[test]
fn resolves_sibling_binary_from_anchor_parent_with_exe_suffix() {
    let tmp = TempDir::new().unwrap();
    let mcp_filename = format!("makakoo-mcp{}", std::env::consts::EXE_SUFFIX);
    let makakoo_filename = format!("makakoo{}", std::env::consts::EXE_SUFFIX);

    for rel in ["target/debug", "target/release", "custom-target/ci-fast"] {
        let profile_dir = tmp.path().join(rel);
        std::fs::create_dir_all(&profile_dir).unwrap();
        let anchor = profile_dir.join(&makakoo_filename);
        let expected = profile_dir.join(&mcp_filename);

        assert_eq!(
            common::sibling_bin_from_anchor(&anchor, "makakoo-mcp"),
            None,
            "missing sibling must return None for {rel}"
        );

        std::fs::write(&expected, b"stub").unwrap();
        assert_eq!(
            common::sibling_bin_from_anchor(&anchor, "makakoo-mcp"),
            Some(expected),
            "present sibling must resolve directly in {rel}"
        );
    }
}

#[test]
fn does_not_walk_ancestors_to_unrelated_build_or_deps_dirs() {
    let tmp = TempDir::new().unwrap();
    let mcp_filename = format!("makakoo-mcp{}", std::env::consts::EXE_SUFFIX);
    let makakoo_filename = format!("makakoo{}", std::env::consts::EXE_SUFFIX);

    // Place a decoy sibling beside an ancestor named `build` and another beside
    // an ancestor named `deps`. When the real profile directory lacks
    // `makakoo-mcp`, lookup MUST return None rather than climbing up.
    let decoy = tmp.path().join(&mcp_filename);
    std::fs::write(&decoy, b"decoy").unwrap();

    for nested in [
        "build/workspace/target/debug",
        "deps/workspace/target/debug",
        "target/debug/build/makakoo/hash/out",
    ] {
        let dir = tmp.path().join(nested);
        std::fs::create_dir_all(&dir).unwrap();
        let anchor = dir.join(&makakoo_filename);
        assert_eq!(
            common::sibling_bin_from_anchor(&anchor, "makakoo-mcp"),
            None,
            "lookup must never climb ancestors from {nested}"
        );
    }
}

#[test]
fn derives_cargo_profile_from_profile_dir_name() {
    assert_eq!(
        common::cargo_profile_for_dir(Path::new("target/debug")),
        Some("dev")
    );
    assert_eq!(
        common::cargo_profile_for_dir(Path::new("target/release")),
        Some("release")
    );
    assert_eq!(
        common::cargo_profile_for_dir(Path::new("target/ci-fast")),
        Some("ci-fast")
    );
}

#[test]
fn path_fallback_requires_opt_in_and_is_disabled_in_ci() {
    let tmp = TempDir::new().unwrap();
    let mcp_filename = format!("makakoo-mcp{}", std::env::consts::EXE_SUFFIX);
    let makakoo_filename = format!("makakoo{}", std::env::consts::EXE_SUFFIX);

    let profile_dir = tmp.path().join("target/debug");
    let path_dir = tmp.path().join("bin-on-path");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::create_dir_all(&path_dir).unwrap();

    let anchor = profile_dir.join(&makakoo_filename);
    let path_bin = path_dir.join(&mcp_filename);
    std::fs::write(&path_bin, b"installed-stub").unwrap();
    let path_env: OsString = std::env::join_paths([&path_dir]).unwrap();

    // Default (no opt-in, not CI): must refuse PATH fallback.
    let err = common::resolve_sibling_bin_with_policy(
        &anchor,
        "makakoo-mcp",
        false,
        false,
        Some(path_env.as_os_str()),
    )
    .unwrap_err();
    assert!(err.contains("MAKAKOO_E2E_USE_PATH=1"), "err: {err}");

    // Opt-in outside CI: uses PATH binary and reports used_path_fallback = true.
    let (resolved, used_path) = common::resolve_sibling_bin_with_policy(
        &anchor,
        "makakoo-mcp",
        true,
        false,
        Some(path_env.as_os_str()),
    )
    .unwrap();
    assert_eq!(resolved, path_bin);
    assert!(used_path);

    // CI set: disables PATH fallback even when opt-in is set, and errors loudly.
    for opt_in in [false, true] {
        let ci_err = common::resolve_sibling_bin_with_policy(
            &anchor,
            "makakoo-mcp",
            opt_in,
            true,
            Some(path_env.as_os_str()),
        )
        .unwrap_err();
        assert!(
            ci_err.contains("CI disables PATH fallback"),
            "err: {ci_err}"
        );
    }

    // Once the workspace sibling exists, it always wins over PATH (even in CI).
    let workspace_bin = profile_dir.join(&mcp_filename);
    std::fs::write(&workspace_bin, b"workspace-stub").unwrap();
    let (resolved, used_path) = common::resolve_sibling_bin_with_policy(
        &anchor,
        "makakoo-mcp",
        true,
        true,
        Some(path_env.as_os_str()),
    )
    .unwrap();
    assert_eq!(resolved, workspace_bin);
    assert!(!used_path);
}
