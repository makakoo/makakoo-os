# Round 3 — drafter


# SPRINT-MAKAKOO-PUBLIC-READY

## Origin

User request: "Make Makakoo OS ready for public marketing by fixing install, docs, CI, site, and cross-OS release gates."

## Pre-Sprint Decisions Required

**Decision 1 — ARM64 Windows: ship or drop?**
Gated before Phase 4b. Check `.github/workflows/release.yml` CI upload step. If `aarch64-pc-windows-msvc` exists → ship. If absent → drop. Decision recorded in `GATE.md` before Phase 4b begins. No "default" — explicit binary choice.

**Decision 2 — Windows install harness exists?**
Phase 1 must confirm `scripts/windows-install-test.ps1` or equivalent harness exists before Phase 3 Windows criterion is achievable. If harness missing, Phase 1 scope expands to include building it.

## Phase Ordering

- Phases 1 and 2 run in parallel (no dependencies between them).
- Phase 3 requires Phase 1 diagnostic output (Windows harness confirmed; failing test name captured).
- Phase 4a runs serially after Phase 3 (depends on confirmed install behavior).
- Phase 4b gated on Decision 1; runs after Phase 4a (shares marketing copy surface).
- Phase 4c runs parallel to 4b (both update marketing copy; no cross-dependency).
- Phase 5 gates merge.

---

### Phase 1: CI and smoke tests green

**Goal:** Restore CI to passing state; fix post-public smoke workflow so it proves real install completion, not false success.

**Criteria:**

- `macos-latest` CI job completes with zero test failures.
- `windows-latest` CI job completes with zero integration test failures.
- Post-public smoke workflow re-run shows install completed without prompting or aborting, and distro/plugin install succeeds on all three OS targets.
- All file paths resolved via audit before any edit is attempted.

**Artifacts/Files/Deliverables:**

- Locate `installer/temp-dir.rs` via `rg -l "temp.*panic\|No such file or directory" installer/` — fix path confirmed by grep results.
- Locate `makakoo/tests/adapter_cli.rs` — run `cargo test --test adapter_cli 2>&1` on windows-latest; capture failing test name and error message.
- Locate `.github/workflows/smoke-public.yml` — audit current abort prompt; confirm whether bug = prompt-abort or missing distro verification.
- Fix `distribution/smoke.yml` only if audit confirms missing verification is a separate bug from prompt-abort; otherwise skip this deliverable.
- Windows install harness (`scripts/windows-install-test.ps1` or equivalent) — confirm exists or build it as prerequisite.

**Tests/Success Metrics:**

- Full CI re-trigger on feature branch; all jobs pass with zero panics on macos-latest and zero failures on windows-latest.
- GitHub Actions re-trigger for Linux and Windows smoke runners; logs confirm install completed without interactive prompts.
- Manual isolated smoke on macOS arm64: `curl -fsSL https://makakoo.com/install.sh | PREFIX=$HOME/isolated sh && makakoo --prefix $HOME/isolated status` exits 0.

---

### Phase 2: Docs verification pass

**Goal:** Eliminate docs failures by running verification tool and fixing only confirmed failures against baseline.

**Criteria:**

- Run `verify-docs` and capture actual output; baseline failure count established from diagnostic run before any fix is scoped.
- Target: zero false-positive failures vs captured baseline.
- All stale command references removed or corrected to match current CLI surface.
- Shell portability issues fixed (POSIX-safe guards, `bash` shebang where required).
- Allowlist (`verify-docs-allowed-future.txt`) created only if documented commands reference explicitly future-planned plugins confirmed by audit; otherwise omitted.

**Artifacts/Files/Deliverables:**

- `docs/` — correct stale command references and broken plugin links identified in verify-docs output.
- `README.md` — verify every `makakoo` subcommand cited exists in `src/cli/`; stale commands listed in verify-docs output corrected.
- `scripts/verify-docs.sh` or equivalent — update assertions for removed commands based on verify-docs diagnostic output.
- `verify-docs-allowed-future.txt` — conditional: only if verify-docs output confirms future-planned plugin references; otherwise omit this file.

**Tests/Success Metrics:**

- `verify-docs` workflow on PR branch: zero false-positive failures against captured baseline.
- `cargo test --workspace` passes (already gated in CI; confirm no regression).

---

### Phase 3: Isolated install proof

**Goal:** Prove full core distro installs without prompts on all three OSes; eliminate `distro core: failed 1 / total 16` gap.

**Criteria:**

- `makakoo install --skip-daemon --skip-infect --yes --no-setup` completes with `distro core: installed 16, skipped 0, failed 0` on macOS, Linux, and Windows x64.
- Diagnostic step: run `cargo build -p lib-harvey-core`; confirm `makakoo-venv-bootstrap` binary exists in build output before scoping the fix.
- If binary exists but PATH issue persists → fix PATH propagation mid-install.
- If binary missing → ensure it is included in build output.
- If ordering confirmed as root cause → reorder `lib-harvey-core` before `agent-browser-harness` in install sequence.

**Artifacts/Files/Deliverables:**

- `distribution/src/core.rs` or equivalent — fix applied based on diagnostic evidence, not assumption.
- `distribution/defaults/core-distro.toml` — reorder plugins if ordering confirmed as root cause.
- Backup of `core-distro.toml` saved to `~/MAKAKOO/development/backups/sprint-makakoo-public-ready-core-distro-toml/` before any edit.
- Windows install harness verified working from Phase 1.

**Tests/Success Metrics:**

- Fresh isolated install on macOS arm64: `makakoo install --yes --no-setup` → `failed 0`.
- Fresh isolated install on Linux x64: same check passes.
- Fresh isolated install on Windows x64 (PowerShell): same check passes via Windows harness.
- All three captured as passing GitHub Actions jobs with explicit `failed 0` in the log.

---

### Phase 4: Marketing surface and signing decision

**Goal:** Ensure all marketing surfaces reflect actual product state; resolve ARM64 ship/drop and signing status explicitly.

**Marketing Surfaces Enumerated:**
Homepage, README.md, `distribution/install.sh` banner, GitHub repo description, social copy (if any).

---

#### Phase 4a: Homepage and link fixes

**Goal:** Correct homepage install command and source links to reflect actual distribution behavior.

**Criteria:**

- Homepage install command uses `curl -fsSL https://makakoo.com/install.sh | sh` and hands off to `makakoo install` / setup wizard.
- All GitHub links resolve to `https://github.com/makakoo/makakoo-os`.
- Confirm `distribution/install.sh` behavior by quoting relevant lines before Phase 4a is marked PASS.

**Artifacts/Files/Deliverables:**

- `site/homepage.html` or equivalent — update install command; fix all GitHub source links to correct org path.
- `distribution/install.sh` — confirmed to invoke `makakoo install` based on quoted source lines.

**Tests/Success Metrics:**

- Homepage loads and install command executes without manual intervention.
- All homepage links resolve to live repositories with correct org path.

---

#### Phase 4b: ARM64 Windows decision

**Goal:** Implement the pre-sprint ARM64 = ship or drop decision; ensure all marketing surfaces are consistent.

**Criteria:**

- Ship path: `aarch64-pc-windows-msvc.zip` added to GitHub Actions upload step; `install.ps1` ARM64 mapping returns HTTP 200 with zip; Phase 4 marketing surfaces include ARM64 claim.
- Drop path: `ARM64` mapping removed from `install.ps1`; Phase 4 marketing surfaces remove ARM64 claims; non-zero exit with "not yet supported" message if ARM64 user runs install.ps1.

**Artifacts/Files/Deliverables:**

- `.github/workflows/release.yml` — add `aarch64-pc-windows-msvc` upload step if ship path chosen.
- `install/install.ps1` — implement or remove ARM64 mapping per decision.
- Marketing surfaces updated per Decision 1: homepage, README.md, GitHub repo description, `install.sh` banner.

**Tests/Success Metrics:**

- `install.ps1` ARM64 flow: either returns HTTP 200 with zip (ship) or prints "not yet supported" and exits non-zero (drop).
- Decision and outcome recorded in `GATE.md`.

---

#### Phase 4c: Signing status

**Goal:** Document actual binary signing state explicitly in all marketing surfaces.

**Criteria:**

- Marketing copy and README make explicit whether binaries are signed (Authenticode + notarization complete) or unsigned developer beta.
- Scope is documentation only. If notarization + Authenticode pipeline does not exist in the repo, scope = "document as unsigned beta."
- Evidence definition: workflow file path + step name + last successful run URL. No claims exceeding documented state.

**Artifacts/Files/Deliverables:**

- `README.md` — add "Signing status" paragraph: either "complete: notarization + Authenticode signed" with pipeline evidence (workflow file + step + run URL) or "unsigned developer beta."
- Marketing surfaces updated with consistent signing claim matching documented state.

**Tests/Success Metrics:**

- README signing paragraph present and matches actual binary state.
- No claims in any marketing surface exceed documented signing reality.

---

### Phase 5: GO/NO-GO gate

**Goal:** Produce final marketing readiness verdict with evidence bundle; scope rollback as separate follow-up.

**Criteria:**

- All Phase 1–4 sub-phase criteria satisfied.
- GitHub Actions run IDs and public URLs recorded for every claim.
- Signing decision explicit in marketing copy and linked from gate document.
- Rollback scoped as separate follow-up: if post-launch smoke regresses within 48h, `GATE.md` updated to NO-GO and P1 patch sprint opened with regression as sole focus (monitoring owner and alert channel defined in GATE.md, not in this sprint).

**Artifacts/Files/Deliverables:**

- `development/sprints/SPRINT-MAKAKOO-PUBLIC-READY/GATE.md` — structured gate doc: each blocker listed, evidence link, PASS/FAIL, GO/NO-GO verdict with rationale.
- Rollback protocol defined in GATE.md: monitoring owner, alert channel, 48h regression trigger, P1 patch sprint procedure.

**Tests/Success Metrics:**

- `GATE.md` exists at sprint path and contains GO or NO-GO verdict.
- If NO-GO: named blocker list present; sprint not considered complete until resolved.
- If GO: PR with marketing copy (updated homepage, README signing status, ARM64 decision) ready to merge.
- All evidence URLs in `GATE.md` resolve and show passing state.
