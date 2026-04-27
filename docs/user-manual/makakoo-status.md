# System status — checking that Makakoo is healthy

There is no single `makakoo status` command. Status information is
distributed across several focused subcommands depending on which subsystem
you are asking about. This page is a quick decision guide.

## Which command to run

| Question | Command |
|---|---|
| Is the binary installed and which version? | `makakoo version` |
| Is the daemon running? | `makakoo daemon status` |
| Are SANCHO tasks registered and running? | `makakoo sancho status` |
| Are all CLI hosts infected and in sync? | `makakoo infect --verify` |
| Are write-access grants active? | `makakoo perms list` |
| Is the Brain FTS5 index healthy? | `makakoo memory stats` |
| Are registered adapters reachable? | `makakoo adapter status` |

## Quick post-install health check

Run these three commands in sequence after a fresh install:

```sh
# 1. confirm version and persona loaded
makakoo version

# 2. confirm daemon is running
makakoo daemon status

# 3. confirm SANCHO tasks are registered
makakoo sancho status
```

All three should exit 0 with non-empty output. If the daemon is not running,
see [`makakoo-daemon.md`](makakoo-daemon.md) for the fix.

## Deeper diagnostics

```sh
# tail the last 50 lines of the daemon log
makakoo daemon logs

# check if any CLI host has drifted from the latest bootstrap block
makakoo infect --verify

# confirm the Brain index has rows
makakoo search "test" -l 1
```

## CI / scripted health check

```sh
# exits 0 only when all three pass
makakoo daemon status \
  && makakoo infect --verify \
  && makakoo mcp --health
```

`makakoo mcp --health` returns `{"ok":true,"tools":N}` and exits 0 when the
MCP server binary is present and lists tools correctly. Useful as a
smoke test in a post-deploy pipeline.

## Related commands

- [`makakoo-daemon.md`](makakoo-daemon.md) — daemon install, restart, and log access
- [`makakoo-sancho.md`](makakoo-sancho.md) — SANCHO task registration and tick control
- [`makakoo-infect.md`](makakoo-infect.md) — verify CLI host infection state
- [`makakoo-mcp.md`](makakoo-mcp.md) — MCP server health endpoint
- [`../troubleshooting/index.md`](../troubleshooting/index.md) — failure playbooks

## Common gotcha

**`makakoo daemon status` says `not installed` but SANCHO tasks appear in `sancho status`.**
This means the daemon process started once (manually or via `daemon run`)
but the auto-start service was never registered. The daemon exited when the
terminal closed and SANCHO is no longer running. Fix: run
`makakoo daemon install` to register the LaunchAgent / systemd unit so the
daemon auto-starts on login and persists across sessions.
