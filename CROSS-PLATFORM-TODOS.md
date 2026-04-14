# Cross-Platform TODOs — makakoo-core-rs

Landed with T18 (wave 6 integration). Status as of 2026-04-14.

## macOS — ✓ fully verified

- `cargo build --release --workspace` green on `x86_64-apple-darwin`.
- `cargo test --workspace` — 409 passed, 0 failed, 1 ignored.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- Release binaries (`makakoo`, `makakoo-mcp`) boot against the user's
  real Brain: search, nursery list, daemon status, infect --dry-run,
  MCP `--health` and `--list-tools` all green.
- Python MCP acceptance suite (T1 oracle) passes 8/8 against both the
  Python reference and the Rust binary — byte-for-byte wire parity.
- Apple Silicon (aarch64-apple-darwin) not validated in this sprint —
  only `x86_64-apple-darwin` is installed in rustup. Cargo should build
  cleanly under Rosetta; needs a real M-series run to confirm.

## Linux — deferred, sysroot not installed

- `x86_64-unknown-linux-gnu` target not added to rustup on the build
  host. Running `cargo check --target x86_64-unknown-linux-gnu` would
  fail at link time anyway (no cross linker). Deferred per T18 spec.
- Known-risky spots to audit under Linux:
  - `makakoo-core/src/platform.rs` — uses `dirs` crate; `makakoo_home()`
    fallback path differs on XDG hosts.
  - `makakoo-core/src/daemon/` (T17) — `launchd` install path is gated
    behind `#[cfg(target_os = "macos")]`. A `systemd --user` branch is
    outlined in code comments but not implemented.
  - `keyring` crate — uses Secret Service on Linux, may require
    `dbus-1` at runtime.
  - File locks (`fs2` crate) — works on Linux but advisory semantics
    differ from macOS flock.

## Windows — deferred, sysroot not installed

- `x86_64-pc-windows-gnu` target not added to rustup. Deferred per
  T18 spec. High-risk spots if a future sprint enables Windows:
  - Every `#[cfg(unix)]` branch in `platform.rs` has no Windows mirror.
  - `daemon/` lacks a Windows service path. The `daemon install`
    subcommand is macOS-launchd-only today.
  - `keyring` routes to Windows Credential Manager — usually fine.
  - File lock semantics (`fs2`) — works but path separators in lock
    file construction (`with_extension("lock")`) need review for
    Windows UNC paths.
  - `infect --global` slot paths hard-code POSIX `~/.config/...`
    layouts; Windows CLI homes (e.g. `%APPDATA%\Claude`) are not
    plumbed into `global_slot.rs`.

## Recommendation for a follow-up sprint

1. Install `x86_64-unknown-linux-gnu` + `x86_64-pc-windows-gnu` targets,
   wire GitHub Actions for both, let real cargo check/build/test surface
   the real failures.
2. Add `target_os` branches to `daemon/`, `platform.rs`, `infect/`.
3. Gate the handful of macOS-specific tests behind `#[cfg(target_os = "macos")]`.
4. Land a CI matrix (mac x86_64, mac aarch64, linux x86_64, windows x86_64).

Estimated effort: 2-3 days for parse-level + test green on all three;
another 3-5 days for full daemon parity on Linux/Windows.
