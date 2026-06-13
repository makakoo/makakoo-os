# Releasing Makakoo OS

Authoritative runbook for cutting a new tagged release.

## Channels we ship to

| Channel | What needs touching | How |
|---|---|---|
| GitHub Release tarballs/zip | `Cargo.toml` workspace version | tag push triggers `.github/workflows/release.yml` |
| `curl \| sh` installer | `distribution/install.sh` + `marketing/makakoo/site/install.sh` | content lives in this repo + the makakoo.com Netlify site (`~/MAKAKOO/marketing/makakoo/site/`) |
| `iwr \| iex` installer (Windows) | `install/install.ps1` + `marketing/makakoo/site/install.ps1` | same |
| Homebrew tap | `distribution/homebrew/makakoo.rb` (source of truth) AND live `traylinx/homebrew-tap/Formula/makakoo.rb` | manual mirror |
| `cargo install` | `Cargo.toml` workspace version | implicit |
| `makakoo upgrade` (end-user self-update) | nothing — auto-detects install method | already wired |

The repo's `distribution/install.sh` and `~/MAKAKOO/marketing/makakoo/site/install.sh` MUST stay byte-identical. Same for `.ps1`. The repo file is the source of truth; copy it to the marketing site at every release.

## Step-by-step

```bash
# 0. Land your sprint commits on `main`. Working tree clean.
NEW=0.1.5
PREV=0.1.4

# 1. Bump the workspace version.
sed -i '' "s/^version = \"$PREV\"/version = \"$NEW\"/" Cargo.toml

# 2. Add a CHANGELOG.md entry under [Unreleased].
$EDITOR CHANGELOG.md

# 3. Bump the source-of-truth Homebrew formula. Leave SHAs as
#    REPLACE_AT_RELEASE_* placeholders — the live tap still serves
#    PREV until step 8 lands.
$EDITOR distribution/homebrew/makakoo.rb

# 4. If installer scripts changed, mirror them into the Netlify site.
cp distribution/install.sh   ~/MAKAKOO/marketing/makakoo/site/install.sh
cp install/install.ps1       ~/MAKAKOO/marketing/makakoo/site/install.ps1

# 5. Commit. Use the repo's existing chore(release) style.
git add Cargo.toml CHANGELOG.md distribution/homebrew/makakoo.rb \
        distribution/install.sh install/install.ps1
git commit -m "chore(release): bump v$PREV → v$NEW"

# 6. Tag and push.
git tag v$NEW
git push origin main
git push origin v$NEW

# 7. Wait for `.github/workflows/release.yml` to finish (5 targets).
gh run watch --exit-status \
  $(gh run list --workflow=release.yml --limit=1 --json databaseId --jq '.[0].databaseId')

# 8. Mirror the Homebrew formula into the live tap.
#    Pull SHAs from the release artifacts:
mkdir -p /tmp/mk-shas && cd /tmp/mk-shas
gh release download v$NEW -R makakoo/makakoo-os --pattern '*.sha256'
for f in *.sha256; do echo "$(basename "$f" .sha256): $(awk '{print $1}' "$f")"; done

#    Edit Formula/makakoo.rb in github.com/traylinx/homebrew-tap with
#    the new version + four real SHAs (replacing REPLACE_AT_RELEASE_*),
#    then commit + push to the tap repo.
#    `brew upgrade traylinx/tap/makakoo` will then serve the new
#    version on the next `brew update`.

# 9. Deploy the Netlify site (only if installer scripts changed).
cd ~/MAKAKOO/marketing/makakoo/site && netlify deploy --prod
curl -fsSL -o /dev/null -w "install.sh: %{http_code}\n" https://makakoo.com/install.sh
curl -fsSL -o /dev/null -w "install.ps1: %{http_code}\n" https://makakoo.com/install.ps1

# 10. Smoke-test the curl-pipe path on a clean machine (or with
#     MAKAKOO_NO_AUTORUN=1 in CI).
curl -fsSL https://makakoo.com/install.sh | bash    # then rerun: makakoo --version
```

## End-user upgrade story

Once a new tag is published, end users get the new version via:

| Their original install | Update command |
|---|---|
| `brew install traylinx/tap/makakoo` | `brew upgrade traylinx/tap/makakoo` |
| `curl … install.sh \| sh` | `makakoo upgrade` (uses the curl-pipe re-install path) |
| `cargo install --path makakoo` | `makakoo upgrade` (re-runs cargo install) |

`makakoo upgrade` auto-detects which of the three install paths created the binary (resolves symlinks, recognises `/usr/local/Cellar/...` Homebrew Cellar paths since v0.1.4) and dispatches the matching update command. Users never need to remember which channel they used.

## Smoke tests

- `.github/workflows/smoke.yml` exercises `install.sh` against pre-release artifacts via `MAKAKOO_LOCAL_TARBALL` (skips download) and sets `MAKAKOO_NO_AUTORUN=1` to keep the install non-interactive.
- `.github/workflows/smoke-public.yml` exercises the public `https://makakoo.com/install.sh` URL after a release.

## Drift checks

Before every release, verify these stay in sync:

```bash
diff distribution/install.sh   ~/MAKAKOO/marketing/makakoo/site/install.sh
diff install/install.ps1       ~/MAKAKOO/marketing/makakoo/site/install.ps1
diff distribution/homebrew/makakoo.rb \
     <(curl -fsSL https://raw.githubusercontent.com/traylinx/homebrew-tap/main/Formula/makakoo.rb)
```
