# Round 2 — drafter


# SPRINT-MAKAKOO-PUBLIC-READY

## Origin

User request: "Make Makakoo OS ready for public marketing by fixing install, docs, CI, site, and cross-OS release gates."

## Pre-Sprint Decision Required

Before Phase 4b begins, one binary decision must be resolved and documented: **ARM64 = ship or drop?**

Decision criterion: If `aarch64-pc-windows-msvc` target already exists in `.github/workflows/release.yml` CI upload step → ship. If not → drop from `install.ps1` + all marketing copy. Default = drop (smaller scope). This decision gates Phase 4b and must be recorded in GATE.md.

---

### Phase 1: CI and smoke tests green

**Goal:** Restore CI to passing state; fix post-public smoke workflow so it proves real install completion, not false success.

**Criteria:**

- `macos-latest` CI job completes with zero test failures (no temp-dir panics).
- `windows-latest` CI job completes with zero integration test failures in `makakoo/tests/adapter_cli.rs`.
- Post-public smoke workflow re-run shows install completed without prompting or aborting, and distro/plugin install succeeds on all three OS targets.
- Diagnostic step: capture actual failing test name and error message from `cargo test --test adapter_cli` stderr on windows-latest before writing any fix; scope of Windows fix = the named failing test only.

**Artifacts/Files/Deliverables:**

- `makakoo/tests/adapter_cli.rs` or equivalent — fix Windows integration test failures. Scope bounded by diagnostic output (failing test name + error) captured first.
- `installer/temp-dir.rs` or equivalent — fix `No such file or directory` panics on macOS. Path confirmed from audit or searched in `installer/` before assuming file name.
- `.github/workflows/smoke-public.yml` or equivalent — harden smoke workflow. Diagnostic step: quote current abort prompt from audit; confirm which bug caused false-success (prompt-abort vs missing distro verification). If prompt-abort: pipe `yes` to `--skip-daemon --skip-infect` flags. If missing distro verification: add explicit post-install distro step.
- `distribution/smoke.yml` or equivalent — add explicit post-install distro verification only if audit confirms missing verification is a separate bug from prompt-abort.

**Checks/Tests/Success Metrics:**

- Full CI re-trigger on feature branch; all jobs pass with zero panics on macos-latest and zero failures in `adapter_cli.rs` on windows-latest.
- Manual isolated smoke on macOS arm64: `curl -fsSL https://makakoo.com/install.sh | PREFIX=$HOME/isolated sh && makakoo --prefix $HOME/isolated status` exits 0.
- GitHub Actions re-trigger for Linux and Windows runners; logs confirm install completed without interactive prompts and distro verification ran.

---

### Phase 2: Docs verification pass

**Goal:** Eliminate false-positive docs failures; all documented commands exist, plugins are real, shell snippets are portable.

**Criteria:**

- `verify-docs` run returns zero false-positive failures. Target: `Total: 128 (or actual total) Pass: 128 Fail: 0`.
- Escape hatch: a `verify-docs-allowed-future.txt` allowlist file may be committed; documented commands referencing explicitly-future plugins are excluded from pass/fail count and annotated in output as "planned". This is not a bug — it is a legitimate state.
- All stale command references removed or corrected to match current CLI surface.
- Shell portability issues fixed (POSIX-safe guards, `bash` shebang where required).
- Non-existent plugin references removed or moved to allowlist.

**Artifacts/Files/Deliverables:**

- `docs/` — correct stale command references and broken plugin links.
- `README.md` — verify every `makakoo` subcommand cited exists in `src/cli/` (quote audit's list of stale commands if provided in audit; if not, run `verify-docs` as first step and capture output).
- `verify-docs/` or `scripts/verify-docs.sh` or equivalent — update assertions for removed commands. Diagnostic step: run `verify-docs` and capture actual output proving failure count; do not assume 115/128 without evidence.
- `verify-docs-allowed-future.txt` — new allowlist file if any documented commands reference future-planned plugins.

**Checks/Tests/Success Metrics:**

- `cargo test --workspace` passes (already gated in CI, confirm no regression).
- `verify-docs` workflow on PR branch: zero false-positive failures (future refs on allowlist excluded by design).

---

### Phase 3: Isolated install proof

**Goal:** Eliminate `distro core: failed 1 / total 16` gap; prove full core distro installs without prompts on all three OSes.

**Criteria:**

- `makakoo install --skip-daemon --skip-infect --yes --no-setup` completes with `distro core: installed 16, skipped 0, failed 0` on macOS, Linux, and Windows x64.
- `agent-browser-harness` plugin no longer triggers `makakoo-venv-bootstrap: command not found`.
- Diagnostic step first: run `cargo build -p lib-harvey-core` and confirm `makakoo-venv-bootstrap` binary exists in build output. If binary exists but PATH issue persists mid-install → root cause is PATH propagation, not ordering. If binary missing → include it in build output. Do not assume dependency ordering fix without diagnostic evidence.

**Artifacts/Files/Deliverables:**

- `distribution/src/core.rs` or equivalent — fix based on diagnostic output. If PATH issue: fix PATH propagation mid-install. If ordering issue: reorder `lib-harvey-core` before `agent-browser-harness`.
- `distribution/defaults/core-distro.toml` or equivalent — verify `lib-harvey-core` appears before `agent-browser-harness` in install order if ordering is confirmed as root cause.
- Backup of `core-distro.toml` saved to `~/MAKAKOO/development/backups/sprint-makakoo-public-ready-core-distro-toml/` before any edit (per CLAUDE.md backup rule).
- `plugins-core/lib-harvey-core/src/` — confirm `makakoo-venv-bootstrap` binary is included in plugin build output after diagnostic.

**Checks/Tests/Success Metrics:**

- Fresh isolated install on macOS arm64 and x64: `makakoo install --yes --no-setup` → `failed 0`.
- Fresh isolated install on Linux x64: same check.
- Fresh isolated install on Windows x64 (PowerShell): same check.
- All three captured as passing GitHub Actions jobs with explicit `failed 0` in the log.

---

### Phase 4: Marketing surface and signing decision

*(Three sub-phases. Phase 4b gated on pre-sprint ARM64 decision. Phase 4c scope = documentation only, not pipeline work.)*

---

#### Phase 4a: Homepage and link fixes

**Goal:** Correct homepage install command and source links.

**Criteria:**

- Homepage install command uses `curl -fsSL https://makakoo.com/install.sh | sh` and handoffs to `makakoo install` / setup wizard.
- All GitHub links resolve to `https://github.com/makakoo/makakoo-os` (not traylinx).
- Diagnostic step: verify `distribution/install.sh` invokes `makakoo install` by quoting lines from install.sh or v0.1.5 tag. If not confirmed in audit, add step to capture install.sh behavior before Phase 4a is considered PASS.

**Artifacts/Files/Deliverables:**

- `site/homepage.html` or equivalent — update install command; fix all GitHub source links.
- `distribution/install.sh` or equivalent — confirm it invokes `makakoo install`.

**Checks/Tests/Success Metrics:**

- Homepage loads and install command executes without manual intervention.
- All homepage links resolve to live repositories with correct org path.

---

#### Phase 4b: ARM64 decision

**Goal:** Implement the pre-sprint ARM64 = ship or drop decision.

**Criteria:**

- Ship path: `aarch64-pc-windows-msvc.zip` added to GitHub Actions upload step; `install.ps1` ARM64 mapping returns HTTP 200 with zip; all marketing copy includes ARM64 claim.
- Drop path: `ARM64` mapping removed from `install.ps1`; all marketing copy removes ARM64 claims; clear "not yet supported" message with non-zero exit if ARM64 user runs install.ps1.

**Artifacts/Files/Deliverables:**

- `.github/workflows/release.yml` or `Makefile` — add `aarch64-pc-windows-msvc` upload step if ship.
- `install/install.ps1` — implement or remove ARM64 mapping per decision.
- All marketing copy — consistent ARM64 claim matching decision.

**Checks/Tests/Success Metrics:**

- `install.ps1` ARM64 flow: either returns HTTP 200 with zip, or prints "not yet supported" and exits non-zero.
- Decision and outcome recorded in GATE.md.

---

#### Phase 4c: Signing status

**Goal:** Document actual binary signing state explicitly in all marketing surfaces.

**Criteria:**

- Marketing copy and README make explicit whether binaries are signed (Authenticode + notarization complete) or unsigned developer beta.
- Scope of this phase is documentation only. If notarization + Authenticode pipeline does not exist in the repo, scope = "document as unsigned beta." Do not imply pipeline work that is not planned.
- If pipeline exists (cite evidence from repo), document it with links.

**Artifacts/Files/Deliverables:**

- `README.md` — add "Signing status" paragraph: either "complete: notarization + Authenticode signed" (cite pipeline evidence) or "unsigned developer beta."
- All marketing copy — consistent signing claim matching documented state.

**Checks/Tests/Success Metrics:**

- README signing paragraph is present and matches actual binary state.
- No claims in marketing copy exceed documented signing reality.

---

### Phase 5: GO/NO-GO gate

**Goal:** Produce final marketing readiness verdict with evidence bundle.

**Criteria:**

- All Phase 1–4 sub-phase criteria satisfied.
- GitHub Actions run IDs and public URLs recorded for every claim (CI pass, docs pass, cross-OS install proof, homepage live state).
- Signing decision explicit in marketing copy and linked from gate document.
- Rollback criterion: if GO issued and post-launch smoke regresses within 48h, GATE.md is updated to NO-GO and a P1 patch sprint is opened immediately with the regression as the sole focus.

**Artifacts/Files/Deliverables:**

- `development/sprints/SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15/GATE.md` — structured gate doc: each blocker listed, evidence link, PASS/FAIL, GO/NO-GO verdict with rationale.
- **⚠️ Write to `$MAKAKOO_HOME/data/reports/` requires Sebastian explicit approval before Phase C. Phase C will not execute this write without sign-off. If approval not granted, GATE.md remains in-repo only and the audit file is not updated.**
- Updated `MAKAKOO/data/reports/makakoo-os-install-readiness-2026-05-15.md` (conditional on Sebastian approval).

**Checks/Tests/Success Metrics:**

- GATE.md exists at sprint path and contains GO or NO-GO verdict.
- If NO-GO: named blocker list present; sprint not considered complete until resolved.
- If GO: PR with marketing copy (updated homepage, README signing status, ARM64 decision) ready to merge.
- All evidence URLs in GATE.md resolve and show passing state.
