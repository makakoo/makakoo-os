# Public readiness gate — FINAL GO

Status: **GO** for public developer-beta marketing as of 2026-05-15.

## Launch posture decision

- Marketing mode for this sprint: **public developer beta** unless signing is completed during execution.
- Unsigned binaries are acceptable only if homepage, README, and installer copy clearly say **unsigned developer beta** and mention macOS Gatekeeper / Windows SmartScreen friction.
- Full mainstream claim (“easy install for every user on every OS”) is blocked until signing is complete or friction is documented in a user-tested path.

## Windows ARM64 decision

Initial decision: **drop Windows ARM64 from public support for this launch unless a working `aarch64-pc-windows-msvc` release asset is added and smoked in this sprint.**

Reason: current release has no `makakoo-aarch64-pc-windows-msvc.zip`, but `install.ps1` maps `ARM64` to that missing target.

Execution rule:
- If team chooses ship: add release build target + asset + smoke proof.
- If team chooses drop: installer must print “Windows ARM64 is not supported in this release” and public copy must say Windows x64 only.

## Rollback / regression protocol

In-sprint scope: define and verify the protocol in this file.

Post-launch trigger:
- Any public install regression within 48h flips this gate to NO-GO.
- Owner: Sebastian / Harvey operator on duty.
- Alert channel: Makakoo Brain journal + GitHub Issue labelled `public-install-regression`.
- Procedure: open P1 patch sprint with one blocker only, link failing run/log, ship patch release, rerun public anonymous smoke.

## Gate table

| Gate | Status | Evidence required |
|---|---:|---|
| Repo public + pushed | PASS | `makakoo/makakoo-os` public; `main` pushed; release `v0.1.6` published |
| Main CI green | PASS | `CI` run `25934374828` on `main` success (`ce6c4bf`) |
| verify-docs green/intentional | PASS | `verify-docs` run `25934374787` on `main` success (`ce6c4bf`) |
| Installer endpoints consistent | PASS | `https://makakoo.com/install` matches `/install.sh`; `/install.ps1` live |
| Real anonymous install smoke | PASS | `Smoke (post-public anonymous)` run `25934418432` success for `0.1.6` |
| Core plugin install complete | PASS | Public smoke enforces `failed 0`; local isolated smoke `installed 17, failed 0` |
| Website source links correct | PASS | Live homepage links to `https://github.com/makakoo/makakoo-os` |
| Windows ARM64 claim resolved | PASS | Windows ARM64 explicitly unsupported in installer and public copy; Windows x64 shipped |
| Signing/beta copy explicit | PASS | Public copy says developer beta / unsigned binaries / Gatekeeper + SmartScreen friction |
| Final release cut after fixes | PASS | Release `v0.1.6` published; Homebrew tap `47839a8`; smoke `25934418432` |
