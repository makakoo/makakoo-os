# Changelog

All notable changes to Makakoo OS are tracked here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added on every tagged release. The GitHub Release workflow at
`.github/workflows/release.yml` also generates per-tag notes automatically
via `generate_release_notes: true` — this file is the curated long-form
complement, focused on user-visible changes and migration notes.

## [Unreleased]

### Added
- `makakoo uninfect` — symmetric inverse of `makakoo infect --global`.
  Strips the bootstrap block from every global CLI slot (or the
  `--target <csv>` subset), deletes infect-created-only files, preserves
  user prose around the block. `--dry-run` previews without writing.
- Shell completion via `makakoo completion <bash|zsh|fish|elvish|powershell>`
  + install guide at `install/completions/README.md`.
- `makakoo plugin enable/disable/update` — soft lifecycle verbs.
- `makakoo distro save` — serialize the live install into a reproducible
  distro TOML pinned by exact version + blake3 per plugin.
- Two new distros: `creator.toml` (writers/streamers/artists) and
  `trader.toml` (market-facing autonomous agents). DoD #8 now 5/5.
- Windows added to the CI test matrix — `windows-latest` joins
  macOS + Ubuntu so `#[cfg(windows)]` code paths get exercised per push.
- Plugin install rejects sancho-task name collisions with native kernel
  handlers. New `InstallError::NativeTaskCollision`.
- `makakoo sancho status` now prints `N registered task(s) (X native +
  Y manifest)` so the split is visible at a glance.

### Changed
- Capability socket env var is `MAKAKOO_SOCKET_PATH` across the whole
  stack (kernel spawn + Rust client + Python client + ABI docs). Prior
  drafts used `MAKAKOO_PLUGIN_SOCKET` in the spawn path, which silently
  broke plugins dialing the socket. Regression test locks the name.
- Release pipeline: cargo-dist `ci = ["github"]`, target set includes
  Linux aarch64 + Windows x86_64. Actual release builds + publishing run
  from `.github/workflows/release.yml` on tag push.

### Deferred to a later release
- Apple notarization + Windows Authenticode signing — awaits signing
  cert acquisition. Runbook in `docs/RELEASE_SIGNING.md`.
- Audit log rotation (100 MB / 7-day retention) — Phase G log-management
  story.
- NetHandler for `net/http|tcp|udp|ws` capability verbs — plugins that
  want kernel-enforced network egress wait for Phase H.4.
- winget submission — `distribution/winget/makakoo.yaml` manifest is
  drafted; PR into `microsoft/winget-pkgs` happens post-v0.1.

## [0.1.0] - YYYY-MM-DD

Placeholder entry. Populated at tag time.
