# Round 1 — drafter


```
# SPRINT-MAKAKOO-PUBLIC-READY

## Origin

User request: "Make Makakoo OS ready for public marketing by fixing install,
docs, CI, site, and cross-OS release gates." Context drawn from audit report at
`~/MAKAKOO/data/reports/makakoo-os-install-readiness-2026-05-15.md`.

## Goal

Fix all hard blockers identified in the install-readiness audit, prove real
cross-OS install completion, update marketing surfaces, and produce a
documented GO/NO-GO marketing gate.

---

### Phase 1: CI and smoke tests green

**Goal:** Restore CI to passing state; fix post-public smoke workflow so it
proves real install completion, not false success.

**Criteria:**
- `macos-latest` CI job completes with zero test failures (no temp-dir panics).
- `windows-latest` CI job completes with zero integration test failures in
  `makakoo/tests/adapter_cli.rs`.
- Post-public smoke workflow re-run shows install completed without prompting
  or aborting, and distro/plugin install succeeds on all three OS targets.

**Files:**
- `makakoo/tests/adapter_cli.rs` — fix Windows integration test failures.
- `installer/temp-dir.rs` or equivalent — fix `No such file or directory`
  panics on macOS.
- `.github/workflows/smoke-public.yml` — harden smoke workflow: pipe `yes` to
  `--skip-daemon --skip-infect` to prevent prompt-then-abort false-green.
- `distribution/smoke.yml` or equivalent — add explicit post-install distro
  verification (list installed plugins, confirm no skipped/failed).

**Tests:**
- Re-trigger full CI on feature branch; all jobs pass.
- Manual isolated smoke on macOS: `curl -fsSL https://makakoo.com/install.sh |
  PREFIX=$HOME/isolated sh && makakoo --prefix $HOME/isolated status` exits 0.
- Manual isolated smoke on Linux and Windows runner via GitHub Actions
  re-trigger; logs confirm `distro core: installed 16, skipped 0, failed 0`.

---

### Phase 2: Docs verification pass

**Goal:** Reduce docs failure rate from 115/128 to zero; all documented
commands are live, plugins/CLIs exist, shell snippets are portable.

**Criteria:**
- `verify-docs` run returns `Total: 128 Pass: 128 Fail: 0`.
- All stale command references (`makakoo daemon restart`, `makakoo status`,
  `makakoo health`) are removed or corrected to match current CLI surface.
- Shell portability issues fixed (POSIX-safe `sh` guards, `bash` shebang where
  required).
- References to non-existent plugins or CLIs are removed or documented as
  future.

**Files:**
- `docs/` — correct all stale command references and broken plugin links.
- `README.md` — verify every `makakoo` subcommand cited actually exists in
  `src/cli/`.
- `verify-docs/` or `scripts/verify-docs.sh` — update assertions for removed
  commands; remove localhost `:18080` hardcoded assumptions.

**Tests:**
- `cargo test --workspace` passes (already gated in CI, confirm no regression).
- `verify-docs` workflow run on PR branch: `128/128` pass.

---

### Phase 3: Isolated install proof (core plugin fix)

**Goal:** Eliminate the `distro core: failed 1 / total 16` gap; prove full
core distro installs without prompts or missing binaries on all three OSes.

**Criteria:**
- `makakoo install --skip-daemon --skip-infect --yes --no-setup` completes
  with `distro core: installed 16, skipped 0, failed 0` on macOS, Linux, and
  Windows x64.
- `agent-browser-harness` plugin no longer triggers
  `makakoo-venv-bootstrap: command not found`.

**Files:**
- `distribution/src/core.rs` (or equivalent) — add `lib-harvey-core` as a
  dependency of `agent-browser-harness` so the venv-bootstrap helper is on
  PATH before the plugin installs.
- `distribution/defaults/core-distro.toml` — verify `lib-harvey-core` is
  listed before `agent-browser-harness` in the install order.
- `plugins-core/lib-harvey-core/src/` — confirm `makakoo-venv-bootstrap`
  binary is included in the plugin build output.

**Tests:**
- Fresh isolated install on macOS arm64 and x64: `makakoo install
  --yes --no-setup` → `failed 0`.
- Fresh isolated install on Linux x64: same check.
- Fresh isolated install on Windows x64 (PowerShell): same check.
- All three captured as passing GitHub Actions jobs with explicit `failed 0`
  in the log.

---

### Phase 4: Marketing surface and signing decision

**Goal:** Correct homepage install command, fix broken source link, resolve
Windows ARM64 asset gap, and document signed/unsigned marketing claim.

**Criteria:**
- Homepage install command uses `/install.sh` and auto-handoffs to
  `makakoo install` / setup wizard, matching `install.sh` behavior.
- Homepage no longer references `https://github.com/traylinx/makakoo-os`.
  All links point to `https://github.com/makakoo/makakoo-os`.
- Windows ARM64 either ships with a `makakoo-aarch64-pc-windows-msvc.zip`
  release asset (added to GitHub Actions upload step), or the claim and
  `ARM64` mapping are removed from `install.ps1` and all marketing copy.
- Marketing copy / README makes explicit whether binaries are signed
  (Authenticode + notarization complete) or unsigned developer beta.

**Files:**
- `site/homepage.html` (or equivalent) — update install command to
  `curl -fsSL https://makakoo.com/install.sh | sh`; fix all GitHub source
  links.
- `install/install.ps1` — remove or implement `ARM64` mapping per decision.
- `distribution/install.sh` — confirm it invokes `makakoo install` (already
  done per v0.1.5 tag, verify no regression).
- `README.md` — add "Signing status" paragraph: notarization + Authenticode
  either "complete" or "in progress — unsigned developer beta".
- `Makefile` or `.github/workflows/release.yml` — add `aarch64-pc-windows-msvc`
  to upload assets if ARM64 is shipped.

**Tests:**
- Homepage loads and install command executes without manual intervention.
- `install.ps1` ARM64 flow either returns HTTP 200 with a zip, or prints a
  clear "not yet supported" message and exits non-zero.
- README signing paragraph is present and matches actual binary state.
- All homepage links resolve to live repositories.

---

### Phase 5: GO/NO-GO gate

**Goal:** Produce a final marketing readiness verdict with evidence bundle.

**Criteria:**
- All Phase 1–4 criteria satisfied.
- GitHub Actions run IDs and public URLs recorded for every claim (CI pass,
  docs pass, cross-OS install proof, homepage live state).
- Signing decision is explicit in marketing copy and linked from the gate
  document.

**Files:**
- `development/sprints/SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15/GATE.md` —
  structured decision artifact: each blocker listed, evidence link, PASS/FAIL,
  GO/NO-GO verdict with rationale.
- `MAKAKOO/data/reports/makakoo-os-install-readiness-2026-05-15.md` — updated
  to reflect post-sprint status.

**Tests:**
- Gate doc exists at sprint path and contains a GO or NO-GO verdict.
- If NO-GO, a named blocker list is present; sprint is not considered
  complete until resolved.
- If GO, a PR with marketing copy (updated homepage, README signing status)
  is ready to merge.
```
