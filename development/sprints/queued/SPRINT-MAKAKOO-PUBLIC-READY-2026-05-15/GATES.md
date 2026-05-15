# Public-readiness gates

These gates define when Makakoo OS can be marketed as public-install-ready.

## Gate 0 — repo/release hygiene

Commands:

```bash
cd /Users/sebastian/makakoo-os
git fetch --all --tags --prune
git status -sb
git rev-list --left-right --count HEAD...@{u}
gh repo view makakoo/makakoo-os --json visibility,defaultBranchRef,url
gh release view v0.1.5 --json tagName,isPrerelease,publishedAt,assets,url
```

Pass:
- `main` has clean worktree before release cut.
- `main` is `0 0` against `origin/main` after final push.
- GitHub repo is public.
- Latest release is non-prerelease after final release cut.
- Release assets match supported OS matrix.

## Gate 1 — installer truth

Commands:

```bash
curl -fsSL https://makakoo.com/install.sh -o /tmp/makakoo-install.sh
curl -fsSL https://makakoo.com/install -o /tmp/makakoo-install-noext.sh
curl -fsSL https://makakoo.com/install.ps1 -o /tmp/makakoo-install.ps1
shasum -a 256 /tmp/makakoo-install.sh distribution/install.sh
shasum -a 256 /tmp/makakoo-install-noext.sh distribution/install.sh
shasum -a 256 /tmp/makakoo-install.ps1 install/install.ps1
```

Pass:
- `/install.sh` and `/install` serve the same current Unix installer, or homepage uses only `/install.sh`.
- Windows endpoint matches repo installer.
- Installer copy matches README/docs promises.

## Gate 2 — true anonymous install smoke

Workflow must run without auth and must pass on macOS, Ubuntu, Windows x64.

Required workflow command inside each job:

```bash
makakoo install --skip-daemon --skip-infect --yes --no-setup
makakoo plugin list
```

Pass:
- No `aborted.` in logs.
- No `failed N` plugin count.
- `agent-browser-harness` either installs successfully or is not part of default core.
- Workflow fails if distro confirmation aborts or any plugin hook fails.

## Gate 3 — core plugin dependency

Pass:
- `agent-browser-harness` install hook can find `makakoo-venv-bootstrap` in a clean installed prefix outside the source checkout.
- Clean isolated smoke installs `core` with `failed 0 / total 16` or the intentionally reduced plugin count if browser harness is removed from core.

## Gate 4 — CI

Commands:

```bash
gh run list --workflow CI --branch main --limit 3
gh run view <latest-ci-run> --json conclusion,jobs
```

Pass:
- Latest `CI` on `main` is success.
- macOS, Ubuntu, Windows test jobs green.
- Clippy jobs green.

## Gate 5 — docs verification

Commands:

```bash
gh run list --workflow verify-docs --branch main --limit 3
gh run view <latest-docs-run> --json conclusion,jobs
```

Pass:
- Latest `verify-docs` on `main` is success, or executable docs manifest intentionally skips non-hermetic examples with documented rationale.
- No stale public commands in README/getting-started/homepage.

## Gate 6 — website/public copy

Pass:
- Homepage GitHub link points to `https://github.com/makakoo/makakoo-os`.
- Homepage install command matches working endpoint.
- Public copy says exact supported matrix.
- If unsigned, public copy says developer beta / unsigned binaries; no “polished production installer” claim.

## Gate 7 — release finalization

Pass:
- Cut fresh release after fixes, not just patched main.
- Homebrew tap bumped to final release.
- Public anonymous smoke passes against final release.
- Final audit report saved under `/Users/sebastian/MAKAKOO/data/reports/`.
