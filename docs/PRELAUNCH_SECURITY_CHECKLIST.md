# Pre-launch security checklist

Run this BEFORE pushing the `v0.1.0` tag. Every step is human-gated — nobody but Sebastian can make the rotation calls. This is the checklist that keeps a public launch from leaking credentials or history.

## 1. Rotate every live key (mandatory)

```sh
# Grep the repo for anything that looks like a live secret:
cd /Users/sebastian/makakoo-os
git grep -iE 'api[_-]?key|secret|token|bearer|pass(word)?|ail[_-]?api' -- ':!docs/RELEASE_SIGNING.md'
```

For every hit:
- If it's a placeholder (`PLACEHOLDER_*`, `your-key-here`) → leave it.
- If it looks real → rotate the upstream key + scrub the repo history (see §3).

Keys to rotate even if none appear in the grep:
- `AIL_API_KEY` — switchAILocal gateway
- `GEMINI_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` — whatever's in `~/.env`
- `POSTGRES_PASSWORD` — local postgres instance (for watchdog-postgres)
- Any Telegram bot token for harveychat / Olibia
- GitHub PATs with write scope (if any were used during the sprint)

Re-store the rotated values:

```sh
makakoo secret set AIL_API_KEY
makakoo secret set GEMINI_API_KEY
# ...
```

OS keyring (Keychain / Secret Service / Credential Manager) is the only storage that's safe to keep local.

## 2. Scrub `.env.live` and friends

```sh
# Confirm nothing with credentials is tracked:
git ls-files | grep -iE '\.env(\.|$)|credentials|secrets?\.(json|yml|yaml)'
```

Expected output: empty.

If the grep returns anything:

```sh
git rm --cached <path>
echo "<path>" >> .gitignore
git commit -m "security: untrack <path>"
```

Then scrub history (§3) if the file ever contained a real value.

## 3. `git filter-repo` history scrub (if needed)

If any commit in the past touched a file that had real credentials:

```sh
# Dry-run first.
git filter-repo --path .env.live --invert-paths --dry-run

# Commit after reviewing the preview.
git filter-repo --path .env.live --invert-paths

# Force-push the scrubbed history.
git push origin main --force-with-lease
git push origin --force --tags   # if any tags referenced scrubbed commits
```

**Before force-pushing:** get explicit confirmation from every collaborator that they have no uncommitted work. A force-push invalidates everyone's local main.

**If the repo is private (current state):** the scrub just prevents leakage after you flip it public. Do this step ANY time before the visibility flip.

## 4. Review GitHub org settings

Before making the org / repo public:

- [ ] Org-level 2FA enforcement is on
- [ ] No deploy keys with write access (check Settings → Deploy keys)
- [ ] No Actions secrets that reference real keys (release signing uses `APPLE_*` + `WINDOWS_*` from the runbook — fine; anything else needs review)
- [ ] Branch protection on `main` — require PR review + status checks
- [ ] Dependabot + CodeQL scanning enabled (free for public repos)
- [ ] Visibility set to Public only after steps 1–3 pass

## 5. Token-bearing files in the release tarballs

Release tarballs bundle only the binaries + `README.md` + `LICENSE`. Confirm:

```sh
# Dry-run the release workflow locally if possible.
# Otherwise, push a test tag to a fork first:
git tag v0.0.0-launch-rehearsal
git push origin v0.0.0-launch-rehearsal   # to a fork repo
```

Download the tarball, inspect `tar -tzf` — nothing should include `.env*`, `plugins.lock`, `~/.makakoo/*`, or anything from the home directory.

## 6. Makakoo-vs-Traylinx split

`makakoo-os` ships MIT at github.com/makakoo. `harvey-os` stays private at github.com/traylinx — it's Sebastian's daily-driver Python tree, includes his personal Brain shape, and is NOT for public consumption. Before launch:

- [ ] `git remote -v` in `makakoo-os/` shows only the `makakoo` org remote
- [ ] No commit in `makakoo-os` references `traylinx` URLs (git log + git grep)
- [ ] `harvey-os` submodule is NOT in `makakoo-os/.gitmodules` (verified)

## 7. Social-media prep

Before the announcement drafts from `docs/ANNOUNCEMENT_DRAFT.md` go live:

- [ ] makakoo.com DNS + HTTPS cert active
- [ ] `makakoo.com/install` serves `install.sh` (SHA matches the repo copy)
- [ ] `makakoo.com/install.ps1` serves the PowerShell version
- [ ] GitHub repo description + topics set
- [ ] OpenGraph preview renders (social-post thumbnail)

## 8. Final smoke

On a fresh laptop / VM:

```sh
curl -fsSL https://makakoo.com/install | sh
makakoo install
makakoo sancho status    # expect 18+ tasks
makakoo brain search test
```

All four commands exit 0. Then tag:

```sh
git tag -s v0.1.0 -m "Makakoo OS v0.1.0"
git push origin v0.1.0
```

The `release.yml` workflow runs, uploads artifacts, publishes the GitHub Release. Verify the Release page has:
- All 5 archive files + corresponding `.sha256` files
- Auto-generated release notes
- Correct tag date

Only AFTER step 8 passes does any content from `docs/ANNOUNCEMENT_DRAFT.md` get posted.
