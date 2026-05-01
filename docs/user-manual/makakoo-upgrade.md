# `makakoo upgrade` — Self-update the kernel + MCP binaries

**Since:** v0.1.3 (2026-05-02). Detects how `makakoo` was installed by inspecting the running binary's path, then dispatches to the matching update command. Replaces the per-method ritual (`cargo install --path …` vs `brew upgrade …` vs re-running `install.sh`) with a single verb.

For task-oriented walkthroughs and the per-method shell commands, see [`docs/upgrade.md`](../upgrade.md). This page is the flag reference.

## Synopsis

```
makakoo upgrade [--dry-run] [--reinfect]
                [--method <cargo|brew|curl-pipe>]
                [--source <path>] [--install-script-url <url>]
                [--only-kernel | --only-mcp]
```

## What it does

1. Resolves the running binary via `canonicalize` (follows symlinks).
2. Maps the path to one of `Cargo` / `Homebrew` / `CurlPipe` / `Unknown`. Dev builds under `target/debug/` or `target/release/` are explicitly classified `Unknown`.
3. Plans the upgrade actions for the detected (or `--method`-overridden) install method.
4. Spawns the actions sequentially. First failure aborts the chain.
5. Captures `makakoo version` before and after. Warns if unchanged.
6. Prints a platform-specific daemon-restart hint.

## Flags

| Flag | What it does |
|---|---|
| `--dry-run` | Print the upgrade plan without spawning anything. Same code path as a real run, so what you see is what would execute. |
| `--reinfect` | After a successful upgrade, run `makakoo infect --verify --repair` to refresh bootstrap fragments in every infected CLI / IDE slot. Useful when a release ships persona / MCP-tool changes. |
| `--method <cargo\|brew\|curl-pipe>` | Override the auto-detector. Rare — needed when you've moved the binary outside its canonical install location, or when multiple methods coexist. |
| `--source <path>` | Cargo upgrades only — point at a local source checkout. Overrides `MAKAKOO_SOURCE_PATH`. Default: `cargo install --git https://github.com/makakoo/makakoo-os --locked --force`. |
| `--install-script-url <url>` | Curl-pipe upgrades only — override `https://makakoo.com/install.sh`. **Refuses non-HTTPS URLs.** |
| `--only-kernel` | Upgrade only the `makakoo` binary; skip `makakoo-mcp`. Mutually exclusive with `--only-mcp`. |
| `--only-mcp` | Upgrade only `makakoo-mcp`; skip the kernel. |

## Environment

| Var | Effect |
|---|---|
| `MAKAKOO_SOURCE_PATH` | Default `--source` for Cargo upgrades. CLI flag wins if both are set. |

## Per-method action plan

Auto-detected from the running binary's path (override with `--method`):

| Method | Detector signal | Action plan |
|---|---|---|
| Cargo | `~/.cargo/bin/makakoo` | `cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo` (and `makakoo-mcp` unless `--only-kernel`). With `--source <path>`: `cargo install --path <path>/makakoo[-mcp] --locked --force`. |
| Homebrew | `/opt/homebrew/bin/`, `/usr/local/bin/`, `/home/linuxbrew/.linuxbrew/bin/` | `brew update && brew upgrade traylinx/tap/makakoo`. |
| Curl-pipe | `$MAKAKOO_PREFIX/bin/` (default `$HOME/.local/bin/`) | `curl -fsSL <install-script-url> \| sh`. URL is HTTPS-only. |
| Unknown | Anything else (custom prefix, dev build, manually-copied binary) | Refuses with an error listing the supported methods. Pass `--method` to force a path, or reinstall via one of the above. |

## Daemon restart

The Makakoo daemon (registered via `makakoo daemon install`) keeps the old binary mapped into memory until it's restarted. v1 of the verb does NOT auto-restart; instead it prints the platform-specific command:

| Platform | Command |
|---|---|
| macOS | `launchctl kickstart -k gui/$UID/com.traylinx.makakoo` |
| Linux (systemd) | `systemctl --user restart makakoo` |

A first-class `makakoo daemon restart` is queued for a follow-up sprint.

## MCP child staleness

When any AI CLI (Claude Code, Gemini, OpenCode, etc.) spawns a `makakoo-mcp` child via stdio, that child runs the binary that was on `PATH` at session-start. **Restart the host CLI session** after `makakoo upgrade` so it spawns a fresh MCP child against the new binary. Without a restart, your CLI keeps talking to the old `makakoo-mcp` forever, even after the upgrade.

## Examples

```bash
# auto-detect + upgrade both binaries (most common)
makakoo upgrade

# preview the plan without executing
makakoo upgrade --dry-run

# upgrade + refresh CLI bootstrap fragments in the same step
makakoo upgrade --reinfect

# force a method when auto-detect picks the wrong one
makakoo upgrade --method brew
makakoo upgrade --method cargo
makakoo upgrade --method curl-pipe

# Cargo upgrade from a local checkout instead of the public repo
makakoo upgrade --source ~/makakoo-os
MAKAKOO_SOURCE_PATH=~/makakoo-os makakoo upgrade

# scope to one binary
makakoo upgrade --only-kernel
makakoo upgrade --only-mcp
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Upgrade planned + executed. Version delta confirmed (or warning printed if unchanged). |
| 1 | Planning failed (Unknown install method, non-HTTPS URL, conflicting flags) OR a spawned action exited non-zero OR the version was unchanged after a non-`--dry-run`. |
| 2 | Invalid flags (e.g. both `--only-kernel` and `--only-mcp`). |

## Out of v1 scope (queued)

- `makakoo daemon restart` first-class subcommand
- Upgrade rollback if a mid-flight action fails
- Beta-channel / release-train selection
- Scheduled auto-upgrade (deliberately omitted — explicit consent is required every time)
- `makakoo version --json` for structured pre/post comparison
- Pre-upgrade test gate (`cargo test` before swapping the binary)

## See also

- [`docs/upgrade.md`](../upgrade.md) — task-oriented upgrade guide with per-method walkthroughs and a troubleshooting matrix.
- [`makakoo install`](../getting-started.md) — initial install (`distro + daemon + infect`).
- [`makakoo plugin update`](makakoo-plugin.md) — update a single plugin from its recorded source.
- [`makakoo docs update`](../docs-mcp.md) — refresh the docs corpus consumed by `makakoo docs-mcp`.
- [`makakoo infect --verify --repair`](makakoo-infect.md) — re-render bootstrap fragments without a binary upgrade. (`makakoo upgrade --reinfect` chains this on top of the binary swap.)
