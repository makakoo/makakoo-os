# Git-Sourced Plugins

Makakoo plugins can live in a git repository — upstream updates flow into your install via `makakoo plugin update`, zero vendoring. This is how `agent-browser-harness` ships (see [browser-harness.md](browser-harness.md)).

Schema reference: `spec/PLUGIN_MANIFEST.md` §3 (locked v0.1).

## TL;DR

```bash
# Install pinned to a semver tag
makakoo plugin install git+https://github.com/user/my-plugin@v0.1.0

# Install from a rolling branch — requires explicit opt-in
makakoo plugin install git+https://github.com/user/my-plugin@main --allow-unstable-ref

# Install from a content-addressed tarball
makakoo plugin install https://example.com/my-plugin-0.1.0.tar.gz --sha256=<hex>
```

## The `[source]` table

A plugin manifest declares its source in one of three shapes:

```toml
# 1. Local path (bundled in plugins-core/).
[source]
path = "plugins-core/my-plugin"

# 2. Git — lives at the named URL + ref.
[source]
git = "https://github.com/me/my-plugin"
rev = "v0.1.0"

# 3. HTTPS tarball — content-addressed.
[source]
tar = "https://example.com/my-plugin-0.1.0.tar.gz"
blake3 = "a8bf…<64 hex>"
```

Exactly one of `path` / `git` / `tar` may be declared.

## Ref pinning rules (locked v0.4)

Git refs MUST be one of:
- A **semver tag** (`/^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$/`).
  Examples: `v0.1.0`, `1.2.3`, `v2.0.0-alpha.1`, `v1.0.0-rc1`.
- A **40-char SHA** (`/^[a-f0-9]{40}$/`).

Anything else (branch names like `main`, `master`, short SHAs) is considered **unstable** and is rejected unless you pass `--allow-unstable-ref`. The warning is printed on install so the non-pin stays visible in logs.

## blake3 hashing

- **Git sources**: `blake3` is optional. The resolved git SHA is already a content pin.
- **Tarball sources**: `blake3` (or `--sha256=<hex>` CLI flag) is **required**. Makakoo refuses to install an unverified archive.

The blake3 in `[source]` is computed over the installed tree post-extract, not the raw archive. If your tarball contains a wrapping directory (GitHub release layout), Makakoo unwraps it before hashing.

## Where files land

Plugin source lives at `$MAKAKOO_HOME/plugins/<name>/` regardless of whether it came from a path / git / tar source. The `source` field in `plugins.lock` preserves provenance:

```toml
[[plugin]]
name = "agent-browser-harness"
version = "0.1.0"
source = "git:https://github.com/user/my-plugin@v0.1.0"
resolved_sha = "abc123…40-char"      # git SHA at install time
manifest_hash = "sha256:…"            # sha of plugin.toml bytes
blake3 = "…"                          # computed blake3 of staged tree
installed_at = "2026-04-21T03:14:00Z"
enabled = true
```

## Publishing your own

1. Write a `plugin.toml` and `install.sh` in a git repo.
2. Push a semver tag.
3. Users install with `makakoo plugin install git+<url>@<tag>`.

Users re-trust on every update where the `manifest_hash` changes (capability drift, sandbox profile change, etc.) — see [update-workflow.md](update-workflow.md).

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `unstable git ref "main" …` | Ref isn't a tag / 40-char SHA | Add `--allow-unstable-ref` or pin to a tag |
| `sha256 mismatch for <url>: expected …, got …` | Tarball bytes changed upstream or wrong hash passed | Verify upstream integrity, update `--sha256=<hex>` |
| `git clone failed for <url>@<ref>: …` | Network, ref missing, auth required | Check network, verify ref exists, set `GIT_ASKPASS` for private repos |
| `venv-bootstrap failed: …` | Python missing, pip error, requirements conflict | Check `$MAKAKOO_VENV_PYTHON`, inspect `<plugin_dir>/.venv.lock` for stale state |
| `[install].unix script for X exited 7: <stderr>` | Plugin's install.sh failed | Fix the underlying issue; re-run `makakoo plugin install` (lock isn't updated on script failure, so retry is safe) |

## Internals

- **Fetcher**: `makakoo-core::source_fetch` (Phase A). Shells to `git` + `curl` — no new Rust deps, zero compile-time burden.
- **Installer dispatch**: `makakoo-core::plugin::install::install()` (Phase B). Dispatches on `PluginSource::{Path, Git, Tarball}`.
- **Lock file**: `$MAKAKOO_HOME/config/plugins.lock`, atomic writes via sibling `lock.tmp + rename`.
