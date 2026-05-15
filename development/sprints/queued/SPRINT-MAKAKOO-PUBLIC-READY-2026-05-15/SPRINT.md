# SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15

## Goal

Make Makakoo OS genuinely ready for public marketing by fixing install completion, CI, docs, website copy, release assets, and cross-OS smoke gates until a final evidence-backed GO/NO-GO verdict can be issued.

## Current verdict

**NO-GO for broad marketing.** Developer-beta messaging is allowed only after this sprint fixes public install blockers or clearly documents remaining friction.

## Source evidence

- Audit report: `/Users/sebastian/MAKAKOO/data/reports/makakoo-os-install-readiness-2026-05-15.md`
- Sprint evidence snapshot: `development/sprints/queued/SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15/EVIDENCE.md`
- Initial gate: `development/sprints/queued/SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15/GATE.md`
- Repo: `/Users/sebastian/makakoo-os`
- Remote: `https://github.com/makakoo/makakoo-os.git`
- Public repo: `https://github.com/makakoo/makakoo-os`
- Current HEAD: `2266d4b docs: sweep installer docs for v0.1.5 wizard auto-handoff`
- Latest release: `v0.1.5`

## Non-negotiable done definition

This sprint is not complete until:

1. Latest `CI` on `main` is green.
2. Latest docs verification is green or has a reviewed hermetic-skip manifest for non-executable docs.
3. Public anonymous install smoke proves real install completion on macOS, Linux, and Windows x64.
4. `makakoo install` cannot abort and still exit success.
5. Core distro installs with zero plugin failures or the default distro intentionally excludes non-hermetic plugins.
6. Homepage/docs/release copy match actual install behavior and support matrix.
7. Windows ARM64 is either shipped and smoked or explicitly unsupported.
8. Signing state is explicit in public copy.
9. A final release is cut after fixes, Homebrew is bumped, and post-release anonymous smoke passes.

## Scope boundaries

In scope:
- Rust CLI/install behavior.
- Core distro dependency/order fixes.
- GitHub Actions CI/smoke/docs workflows.
- README/docs/homepage/source-link corrections.
- Release workflow matrix/support-matrix corrections.
- Final GO/NO-GO report.

Out of scope unless explicitly chosen during Phase 4:
- Apple notarization and Windows Authenticode implementation. If not implemented, public copy must say unsigned developer beta.
- Windows ARM64 support. Default sprint stance is drop/unsupported unless adding and smoking the target is selected.

## Locked decisions

### LD-1 — Marketing posture

Until signing is proven, public copy must say **developer beta / unsigned binaries**. This is not optional. Avoid “frictionless every user” language.

### LD-2 — Windows ARM64

Default launch support is **Windows x64 only**. Windows ARM64 must be removed from installer claims unless this sprint adds `aarch64-pc-windows-msvc` release assets and smoke coverage.

### LD-3 — Smoke tests must fail on aborts

Any log containing `aborted.` during install is failure. Any non-zero plugin failure count is failure. “install complete” is not sufficient.

### LD-4 — Evidence before edits

If a phase references a file path, the file must be verified in `EVIDENCE.md` or discovered at phase start. If discovery fails, phase updates `GATE.md` with the real path/blocker before editing.

---

### Phase 1: Fix install false-success and smoke gate truth

**Goal:** Make `makakoo install` and public smoke workflows fail honestly when distro install aborts or plugins fail.

**Diagnostic facts already known:**

- Existing files:
  - `makakoo/src/commands/install.rs`
  - `makakoo/src/commands/distro.rs`
  - `.github/workflows/smoke-public.yml`
  - `.github/workflows/smoke.yml`
- Current false-green evidence:
  - Post-public smoke run `25922943010` is green while logs show `Proceed? [y/N]`, `aborted.`, then `install complete`.

**Artifacts/Files/Deliverables:**

- `makakoo/src/commands/install.rs`
  - Propagate distro install abort/failure as non-zero result.
  - Do not print `install complete` if a required step aborted.
- `makakoo/src/commands/distro.rs`
  - Ensure user-declined prompt returns a distinguishable non-success result.
- `.github/workflows/smoke-public.yml`
  - Run `makakoo install --skip-daemon --skip-infect --yes --no-setup`.
  - Assert no `aborted.` in logs.
  - Assert plugin failure count is zero.
- `.github/workflows/smoke.yml`
  - Same real-install assertions for pre-public candidate smoke.
- Tests for abort semantics in existing Rust test modules or new focused test.

**Checks/Tests/Success Metrics:**

- Local/unit: targeted cargo test covering declined distro confirmation returns failure.
- Workflow logs no longer contain `aborted.` on green runs.
- If `makakoo install` aborts in CI, workflow exits non-zero.
- Smoke workflow command includes `--yes --no-setup` and validates plugin install result.

---

### Phase 2: Fix core distro plugin dependency and isolated install completion

**Goal:** Make a clean installed prefix outside the source checkout install the default core distro with zero plugin failures.

**Diagnostic facts already known:**

- Existing files:
  - `distros/core.toml`
  - `plugins-core/agent-browser-harness/install.sh`
  - `plugins-core/agent-browser-harness/plugin.toml`
  - `plugins-core/lib-harvey-core/bin/makakoo-venv-bootstrap`
- Reproduced failure:
  - `agent-browser-harness` install hook fails with `makakoo-venv-bootstrap: command not found`.

**Artifacts/Files/Deliverables:**

- One of these fixes, selected by evidence and documented in commit message:
  - Add `lib-harvey-core` to `distros/core.toml` before `agent-browser-harness`; or
  - Ensure plugin install PATH includes bundled `plugins-core/lib-harvey-core/bin`; or
  - Replace harness install hook dependency with a direct `makakoo plugin internal venv-bootstrap` invocation; or
  - Remove `agent-browser-harness` from default `core` if its install is not hermetic enough for public default.
- `distros/core.toml`
  - Updated only if dependency/order/default-distro change is selected.
- `plugins-core/agent-browser-harness/install.sh`
  - Updated only if hook invocation is selected.
- `GATE.md`
  - Records selected root cause and fix path.

**Checks/Tests/Success Metrics:**

- Clean isolated macOS install outside source checkout:
  - `curl -fsSL https://makakoo.com/install.sh | bash` or local candidate artifact equivalent.
  - `makakoo install --skip-daemon --skip-infect --yes --no-setup`.
  - Expected: `failed 0` and no `makakoo-venv-bootstrap: command not found`.
- GitHub public smoke reproduces same on macOS, Ubuntu, Windows x64.
- `makakoo plugin list` shows expected default distro plugins installed or intentionally omitted.

---

### Phase 3: Fix red CI on macOS and Windows

**Goal:** Restore the main CI workflow to green across macOS, Ubuntu, and Windows.

**Diagnostic facts already known:**

- Current failing run: `https://github.com/makakoo/makakoo-os/actions/runs/25544289474`
- macOS failure: many temp-dir `No such file or directory` panics; `954 passed; 152 failed`.
- Windows failure: `makakoo/tests/adapter_cli.rs`, `7 failed`.
- Existing files likely involved:
  - `makakoo-core/src/platform.rs`
  - `makakoo-core/src/superbrain/recall.rs`
  - `makakoo-core/src/superbrain/store.rs`
  - `makakoo-core/src/swarm/artifacts.rs`
  - `makakoo/tests/adapter_cli.rs`
  - `makakoo-core/src/adapter/install.rs`

**Artifacts/Files/Deliverables:**

- Fix for temp-dir lifetime/path bug on macOS.
- Fix for Windows adapter CLI integration test failures.
- Any test fixture updates needed for cross-platform path separators.
- `GATE.md` updated with failing run IDs, fixed commit, and green run ID.

**Checks/Tests/Success Metrics:**

- Local macOS targeted test reproduces/fixes temp-dir bug where possible.
- CI `test (macos-latest)` passes.
- CI `test (windows-latest)` passes.
- CI `test (ubuntu-latest)` remains green.
- Clippy jobs remain green.
- Latest `CI` run on `main` after merge is success.

---

### Phase 4: Fix docs verification and public documentation truth

**Goal:** Make docs executable where they claim to be executable, and clearly mark non-hermetic examples so verify-docs is meaningful.

**Diagnostic facts already known:**

- Current failing run: `https://github.com/makakoo/makakoo-os/actions/runs/25544289499`
- Current failure summary: `Total: 128 Pass: 13 Fail: 115`.
- Existing files:
  - `.github/workflows/verify-docs.yml`
  - `ci/verify-docs.sh`
  - `ci/run_on_clean.sh`
  - `ci/docs_manifest.toml`
  - `README.md`
  - `docs/getting-started.md`

**Definition of false-positive doc failure:**

A verify-docs failure is false-positive only if the documented command intentionally requires a non-CI external dependency (for example Chrome GUI, installed AI CLI, live local LLM gateway, user secret, or paid service) and the doc block is explicitly marked non-hermetic/skip in `ci/docs_manifest.toml` with a reason.

All other failures are real doc or product bugs.

**Artifacts/Files/Deliverables:**

- `ci/docs_manifest.toml`
  - Add/adjust skip metadata only for non-hermetic examples with reasons.
- `docs/getting-started.md`, `docs/quickstart.md`, walkthrough docs, and relevant agent docs
  - Replace stale commands with current CLI surface.
  - Remove/mark commands requiring unavailable plugins or CLIs.
  - Fix shell portability (`bash` vs `sh`, macOS-only `open -a`, Linux `xdg-open`).
- `README.md`
  - Align install instructions with actual live endpoints and support matrix.
- `GATE.md`
  - Records latest verify-docs run URL and outcome.

**Checks/Tests/Success Metrics:**

- `verify-docs` workflow on branch is green, or all remaining skips are explicit in manifest and accepted.
- Latest `verify-docs` on `main` after merge is success.
- No README/getting-started command references non-existent `makakoo` subcommands.

---

### Phase 5: Fix website, support matrix, signing/beta copy, and release surfaces

**Goal:** Make every public surface tell the same truth users will experience during install.

**Diagnostic facts already known:**

- Homepage currently shows `curl -fsSL https://makakoo.com/install | sh`.
- Live `/install` differs from `/install.sh` and prints old next steps.
- Homepage source link points to missing `https://github.com/traylinx/makakoo-os`.
- Correct repo is `https://github.com/makakoo/makakoo-os`.
- `install.ps1` has ARM64 mapping, but `v0.1.5` has no Windows ARM64 asset.
- Signing/notarization/AuthentiCode are pending according to README.
- Website source path is not yet identified in `/Users/sebastian/makakoo-os`; phase starts by locating/deploying the site source.

**Artifacts/Files/Deliverables:**

- Website source/deploy repo path recorded in `GATE.md`.
- Homepage install command fixed to current endpoint:
  - Either make `https://makakoo.com/install` identical to current `distribution/install.sh`, or change homepage to `https://makakoo.com/install.sh`.
- Homepage source link fixed to `https://github.com/makakoo/makakoo-os`.
- `install/install.ps1`
  - Either remove ARM64 mapping with explicit unsupported message, or add release support and smoke proof.
- `.github/workflows/release.yml`
  - Updated only if shipping Windows ARM64.
- `README.md`, `docs/getting-started.md`, homepage copy
  - Support matrix: macOS arm/x64, Linux arm/x64, Windows x64; Windows ARM64 only if shipped.
  - Signing/beta state: “unsigned developer beta” unless signing pipeline evidence exists.
- Homebrew tap update path documented for final release.

**Checks/Tests/Success Metrics:**

- `curl -fsSL https://makakoo.com/` shows correct repo link and current install command after deploy.
- `curl -fsSL https://makakoo.com/install` and/or `/install.sh` matches documented command.
- `install.ps1` behavior for ARM64 is deterministic: either HTTP 200 asset or clear unsupported error.
- Public copy never claims signed/notarized unless release artifacts are actually signed.

---

### Phase 6: Cut final release and issue GO/NO-GO evidence report

**Goal:** Ship the fixed install path and produce the final marketing decision.

**Artifacts/Files/Deliverables:**

- New release tag after fixes, not reusing `v0.1.5`.
- GitHub Release assets for exact supported OS matrix.
- Homebrew tap bumped to new release.
- `GATE.md` updated from initial NO-GO to final GO or remaining NO-GO.
- Final audit report saved under `/Users/sebastian/MAKAKOO/data/reports/`.
- Brain journal entry with final verdict.

**Checks/Tests/Success Metrics:**

- Latest `CI` on `main`: success.
- Latest `verify-docs` on `main`: success.
- Release workflow for new tag: success.
- Post-public anonymous smoke against new release: success on macOS, Ubuntu, Windows x64.
- Manual check of public homepage install command and source links passes.
- If final verdict is GO: marketing may say “public developer beta, installable on macOS/Linux/Windows x64.”
- If final verdict is NO-GO: report lists exact remaining blockers and marketing stays blocked.

## Execution notes

- Use specific-file staging only: `git add <file> ...`, never `git add .`.
- No stash.
- Do not cut release until gates are green on `main`.
- Do not mark the sprint complete based on local-only proof; public anonymous smoke is required.
