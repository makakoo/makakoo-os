# Public readiness gate — INITIAL NO-GO

Status: **NO-GO** until all sprint phases pass.

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
| Repo public + pushed | PASS | `git status -sb`, `gh repo view`, final release URL |
| Main CI green | FAIL | Latest `CI` run on `main` success |
| verify-docs green/intentional | FAIL | Latest `verify-docs` run success or documented hermetic skip list |
| Installer endpoints consistent | FAIL | `/install` and `/install.sh` match, or homepage only uses `/install.sh` |
| Real anonymous install smoke | FAIL | No `aborted.`, `--yes --no-setup`, plugin failure count zero |
| Core plugin install complete | FAIL | `distro core: installed N, failed 0` on macOS/Linux/Windows x64 |
| Website source links correct | FAIL | Homepage source link resolves to `makakoo/makakoo-os` |
| Windows ARM64 claim resolved | FAIL | Either shipped/smoked or explicitly unsupported |
| Signing/beta copy explicit | FAIL | README/homepage/install copy consistent |
| Final release cut after fixes | FAIL | New release after all fixes + Homebrew bump + smoke proof |
