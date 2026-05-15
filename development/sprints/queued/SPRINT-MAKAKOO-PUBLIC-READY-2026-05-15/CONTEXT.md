# Context — Makakoo OS public install readiness sprint

User wants to publish marketing posts about Makakoo OS, but only if users can install easily on macOS, Windows, and Linux.

Audit report source: `/Users/sebastian/MAKAKOO/data/reports/makakoo-os-install-readiness-2026-05-15.md`.

## Current repo truth

- Repo: `/Users/sebastian/makakoo-os`
- Remote: `https://github.com/makakoo/makakoo-os.git`
- Public GitHub repo: `https://github.com/makakoo/makakoo-os`
- Branch: `main`
- HEAD: `2266d4b docs: sweep installer docs for v0.1.5 wizard auto-handoff`
- Upstream divergence: `0 ahead / 0 behind origin/main`
- Latest release: `v0.1.5`, published `2026-05-08T06:42:43Z`
- Tag commit: `ff923c8 chore(release): v0.1.5 — installer auto-launches setup wizard`
- Main contains release tag but has two post-tag commits.

## What is already good

- Repo is public.
- Release `v0.1.5` exists and is not prerelease.
- Release assets exist for:
  - `aarch64-apple-darwin`
  - `x86_64-apple-darwin`
  - `aarch64-unknown-linux-gnu`
  - `x86_64-unknown-linux-gnu`
  - `x86_64-pc-windows-msvc`
- Live `https://makakoo.com/install.sh` matches `distribution/install.sh` and downloads public release assets.
- Live `https://makakoo.com/install.ps1` matches repo `install/install.ps1`.
- Homebrew tap formula is live at `traylinx/homebrew-tap`, version `0.1.5`, and matches repo source-of-truth.
- Manual anonymous smoke of live `install.sh` on local Mac installed `makakoo 0.1.5`, `makakoo-mcp`, `distros/`, and `plugins-core/` into an isolated prefix.
- Post-public anonymous smoke workflow triggered on 2026-05-15 completed success across Ubuntu, macOS, Windows x64: run `25922943010`.

## Hard blockers found

1. Latest `main` CI is red.
   - CI run `25544289474` failed on `macos-latest` and `windows-latest`.
   - macOS: `954 passed; 152 failed`, many temp-dir `No such file or directory` panics.
   - Windows: `makakoo/tests/adapter_cli.rs` integration tests failed (`7 failed`).

2. Docs verification is red.
   - verify-docs run `25544289499` failed.
   - `Total: 128 Pass: 13 Fail: 115`.
   - Failures include stale commands (`makakoo daemon restart`, `makakoo status`, `makakoo health`), missing plugins, missing CLIs, shell portability issues, localhost `:18080` assumptions.

3. Post-public smoke is false-green.
   - Run `25922943010` says success, but logs show `makakoo install --skip-daemon --skip-infect` prompted `Proceed? [y/N]`, read no input, printed `aborted.`, then still exited success and printed `install complete`.
   - This does not prove distro/plugin install completed.

4. True isolated install with `--yes --no-setup` has a core plugin failure.
   - Command: live `install.sh`, isolated prefix/home, then `makakoo install --skip-daemon --skip-infect --yes --no-setup`.
   - Result: `distro core: installed 15, skipped 0, failed 1 / total 16`.
   - Failing plugin: `agent-browser-harness`.
   - Root cause reproduced: `install.sh: line 56: makakoo-venv-bootstrap: command not found`.
   - Cause: `agent-browser-harness` relies on helper from `lib-harvey-core`, but core distro does not install `lib-harvey-core` before/with it or otherwise expose helper on PATH.

5. Homepage install command is stale.
   - Homepage shows `curl -fsSL https://makakoo.com/install | sh`.
   - Live `/install` is not the same as `/install.sh`.
   - `/install` does not auto-run `makakoo install`; it prints older next steps (`makakoo infect --global`, `makakoo daemon install`).
   - Docs/README promise auto hand-off through `makakoo install` / setup wizard.

6. Homepage source link is broken.
   - Homepage links `https://github.com/traylinx/makakoo-os`.
   - That repo does not resolve.
   - Correct repo is `https://github.com/makakoo/makakoo-os`.

7. Windows ARM64 path is advertised/implemented but release asset is missing.
   - `install.ps1` maps `ARM64` to `aarch64-pc-windows-msvc`.
   - Release `v0.1.5` has no `makakoo-aarch64-pc-windows-msvc.zip`.
   - Windows ARM users get a 404.

8. Signing still pending.
   - README explicitly says Apple notarization + Authenticode are runbook-ready but certs pending.
   - Need decide marketing stance: developer beta without signing vs wait for signing.

## Sprint objective

Create and execute a public-readiness sprint that fixes all hard blockers, proves real install completion on macOS/Linux/Windows, updates site/docs/release surfaces, and produces a final GO/NO-GO marketing gate with evidence.

## Constraints

- Do not overclaim. Marketing is blocked until gates are green.
- Surgical commits only: `git add <specific_file>`.
- No stash.
- All public install claims must be backed by live public URLs and GitHub Actions evidence.
- Cross-OS support must be explicit: macOS arm/x64, Linux arm/x64, Windows x64; Windows ARM64 either shipped or removed from claim/installer.
- Signing can be deferred only if public copy says developer beta / unsigned binaries clearly.
- Verify claims live, not by reading docs only.
