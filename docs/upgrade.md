# Upgrading Makakoo

`makakoo upgrade` self-updates the `makakoo` + `makakoo-mcp` binaries by detecting how you installed Makakoo and running the matching update command.

```
makakoo upgrade [--dry-run] [--reinfect] [--method ...] [--source ...] [--install-script-url ...] [--only-kernel] [--only-mcp]
```

## What it does

1. **Detects the install method** by looking at where the running `makakoo` binary lives:
   - `~/.cargo/bin/makakoo` → **Cargo**
   - `/opt/homebrew/bin/makakoo`, `/usr/local/bin/makakoo`, or `/home/linuxbrew/.linuxbrew/bin/makakoo` → **Homebrew**
   - `$HOME/.local/bin/makakoo` (or `$MAKAKOO_PREFIX/bin/makakoo`) → **curl-pipe** (`install.sh`)
   - Anything else (including `target/debug/` dev builds) → **Unknown** — the verb refuses and points you at the right install path.
2. **Plans the upgrade actions** — printed line-by-line so you see exactly what will run.
3. **Spawns the actions sequentially** (skip with `--dry-run`).
4. **Compares versions** before and after; warns if unchanged.
5. **Prints a daemon-restart hint** so any running daemon picks up the new binary.

## Quick reference

```bash
# upgrade everything via the auto-detected method
makakoo upgrade

# preview without spawning
makakoo upgrade --dry-run

# also refresh bootstrap fragments in every infected CLI
makakoo upgrade --reinfect

# force a specific method (e.g. you have both brew and cargo installs)
makakoo upgrade --method brew
makakoo upgrade --method cargo
makakoo upgrade --method curl-pipe

# upgrade Cargo install from a local checkout instead of the public repo
makakoo upgrade --source /path/to/makakoo-os
MAKAKOO_SOURCE_PATH=/path/to/makakoo-os makakoo upgrade

# upgrade only one binary
makakoo upgrade --only-kernel
makakoo upgrade --only-mcp
```

## What runs per install method

### Cargo

By default, `makakoo upgrade` pulls fresh source from the public repo:

```bash
cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo
cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo-mcp
```

Pass `--source <path>` (or set `MAKAKOO_SOURCE_PATH=<path>`) to upgrade from a local checkout instead:

```bash
cargo install --path <path>/makakoo --locked --force
cargo install --path <path>/makakoo-mcp --locked --force
```

### Homebrew

```bash
brew update
brew upgrade traylinx/tap/makakoo
```

The Homebrew tap lives at `github.com/traylinx/homebrew-tap`. New tagged releases are published to the tap automatically by the release workflow.

### curl-pipe

```bash
curl -fsSL https://makakoo.com/install.sh | bash
```

Override the URL with `--install-script-url <url>` if needed. **The verb refuses non-HTTPS URLs.**

### VPS / server quick path

SSH into the server and run the same upgrade verb. Most headless installs use the curl-pipe layout under `$HOME/.local`, so the method is usually `curl-pipe`:

```bash
ssh makakoo-vps
export MAKAKOO_HOME=$HOME/MAKAKOO HARVEY_HOME=$HOME/MAKAKOO
export PATH=$HOME/.local/bin:$PATH

makakoo upgrade --dry-run
makakoo upgrade --method curl-pipe --reinfect
makakoo daemon restart

makakoo --version
makakoo-mcp --version
makakoo infect --verify --json
```

Restart any already-open AI CLI sessions after this so they spawn the new `makakoo-mcp` child and reread the refreshed bootstrap.

### Unknown

If the binary lives somewhere the detector doesn't recognize (custom prefix, manually-copied binary, or a dev build under `target/`), the verb prints an actionable error listing the supported methods and exits 1.

## Daemon restart

The Makakoo daemon (auto-start service registered via `makakoo daemon install`) keeps running with the old binary loaded into memory until you restart it. After a successful upgrade, `makakoo upgrade` prints the restart command to do this:

- **macOS:** `makakoo daemon restart`
- **Linux (systemd):** `makakoo daemon restart`

`makakoo upgrade` does not auto-restart the daemon automatically. Run the printed `makakoo daemon restart` command if a daemon is installed or running; it re-registers the service descriptor so Homebrew/installer path changes are picked up.

## MCP child staleness

When any AI CLI (Claude Code, Gemini, OpenCode, etc.) spawns a `makakoo-mcp` child via stdio, that child runs the binary that was on `PATH` at session-start. **Restart the host CLI session** after `makakoo upgrade` so it spawns the new MCP child. Without a restart, your CLI keeps talking to the old `makakoo-mcp` binary forever, even after the upgrade.

## Re-infect after upgrade

If a release ships new bootstrap fragments (Harvey persona changes, new MCP tool names, slot-format additions), pass `--reinfect` to refresh every infected CLI's global slot:

```bash
makakoo upgrade --reinfect
```

This runs `makakoo infect --global` after the binary swap completes, then verifies with `makakoo infect --verify`. Most upgrades do not need it, but use it when the release notes mention CLI bootstrap, caveman, Headroom, persona, or MCP-tool instruction changes.

## Verifying the upgrade

```bash
makakoo version
```

The first line shows `makakoo X.Y.Z (gitsha)` — confirm the version bumped from what was installed before. `makakoo upgrade` already prints this automatically; the manual command is for after a CLI restart.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `Unknown install` error | Binary under custom prefix or dev build | Reinstall via cargo / brew / install.sh, or pass `--method` to override |
| Version unchanged after upgrade | Already at latest | No action needed; the warning is informational |
| `cargo install` fails with non-exhaustive match errors | Pulling main branch with unfinished feature work | Pin to a specific tag: `cargo install --git https://github.com/makakoo/makakoo-os --tag <release> ...` |
| `brew upgrade` says no formula | `traylinx/tap` not added | `brew tap traylinx/tap && makakoo upgrade` |
| MCP tools still showing old behavior after upgrade | Host CLI is still spawning the old child | Restart the host CLI session |
| Daemon still running old binary | Daemon process kept the old executable mapped | Run the printed `launchctl` / `systemctl` command |

## See also

- `makakoo install` — initial install (distro + daemon + infect)
- `makakoo plugin update <name>` — update a single plugin from its recorded source
- `makakoo docs update` — refresh the docs corpus consumed by `makakoo docs-mcp`
- `makakoo infect --global` — re-render bootstrap fragments without a binary upgrade
