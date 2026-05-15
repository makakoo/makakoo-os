# Lope negotiation escalated

- Reason: 3 NEEDS_FIX rounds exhausted without PASS
- Phase: 0 negotiation-round-3
- Last verdict: NEEDS_FIX
- Confidence: 0.68
- Rationale: Ensemble (2 validators): PASS=0 NEEDS_FIX=2 FAIL=0. Primary (pi): Proposal is well-structured (gated decisions, diagnostic-first approach, parallel phases, rollback protocol) but relies entirely on in-prompt assertions about file/tool existence with zero evidence. T

## Required fixes

- Phase 4c: state explicitly whether signing is a launch blocker or accepted-as-beta. "Document as unsigned" is not a decision — public marketing of unsigned Windows/macOS binaries has SmartScreen/Gatekeeper UX consequences. Add criterion: "if unsigned, homepage + install.sh banner show beta warning."
- Phase 5: resolve contradiction. Either rollback protocol (owner, channel, 48h trigger) is in-sprint scope (define here) or out-of-sprint (don't require GATE.md to contain it). Pick one.
- Pre-Sprint Decision 1: "check release.yml for aarch64-pc-windows-msvc" tests current state, not ship/drop intent. Add criterion: absent ≠ auto-drop; ask whether adding the target is in-scope before defaulting to drop.
- Phase 1: "Locate via rg" assumes paths exist. Add explicit branch: if rg returns zero matches, Phase 1 scope question reopens (is the bug elsewhere?). Don't assume the grep pattern matches reality.
- Phase 3: three diagnostic branches (PATH / missing binary / ordering) but no criterion for which evidence selects which branch. Add: "diagnostic output quoted in commit message before fix applied."
- Phase 2: "zero false-positive failures vs baseline" — define false-positive. Without definition this is unfalsifiable.
- Marketing Surfaces Enumerated includes "social copy (if any)" — drop the hedge or list the surfaces. "If any" defers the audit.
- MISSING_EVIDENCE: All referenced file paths assume existence — `installer/temp-dir.rs`, `makakoo/tests/adapter_cli.rs`, `.github/workflows/smoke-public.yml`, `scripts/windows-install-test.ps1`, `distribution/src/core.rs`, `distribution/defaults/core-distro.toml`, `site/homepage.html`. No evidence any of these exist. Add existence confirmation before Phase begins or mark as ASSUMED and expand Phase 1 scope to audit-first.
- MISSING_EVIDENCE: `verify-docs` tool referenced throughout Phase 2 but no evidence it exists, what it outputs, or that a baseline has been captured. Without this, "fix only confirmed failures vs baseline" is unverifiable.
- MISSING_EVIDENCE: `lib-harvey-core` package and `makakoo-venv-bootstrap` binary assumed to exist in Phase 3 diagnostic step. No evidence provided.
- ASSUMPTION: Phase 3 fix strategy (PATH propagation, binary inclusion, ordering) is conditional but all three paths assume they are the possible root causes. No evidence ordering issue is even plausible — add confirmation that `agent-browser-harness` depends on output from `lib-harvey-core`.
- GATE.md: Referenced in Decision 1 as "decision recorded in `GATE.md`" but Phase 5 is where GATE.md is defined as a deliverable. Circular dependency — move GATE.md creation to Phase 1 or define the recording mechanism separately.
- SCOPE_CURIOUS: Phase 1 "audit current abort prompt" suggests prompt-abort bug exists but Phase 1 does not include fixing it — only diagnosing. If prompt-abort blocks install completion, Phase 1 fix must be in scope or Phase 3 cannot succeed.

## Raw validator feedback

Ensemble (2 validators): PASS=0 NEEDS_FIX=2 FAIL=0. Primary (pi): Proposal is well-structured (gated decisions, diagnostic-first approach, parallel phases, rollback protocol) but relies entirely on in-prompt assertions about file/tool existence with zero evidence. T
