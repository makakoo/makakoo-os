# `makakoo daemon` — CLI reference

The Makakoo daemon is the persistent background process that powers the
SANCHO task engine, the Brain FTS5 watcher, the mascot schedules, and the
MCP HTTP server (when enabled). On macOS it runs as a LaunchAgent; on
Linux it runs as a systemd user unit. `makakoo install` sets it up
automatically — `makakoo daemon` is how you inspect, restart, or remove it
after that.

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `daemon install` | Register the auto-start service for the current user (LaunchAgent / systemd-user unit). |
| `daemon uninstall` | Remove the auto-start service. Does not stop a currently-running daemon. |
| `daemon status` | Print `running / installed / not installed` plus the daemon PID when running. |
| `daemon logs [-l N]` | Tail the last N lines of the daemon log (default: 50). |
| `daemon run` | Run the daemon in the foreground. This is what the OS auto-start hook invokes; also useful for debugging — hit Ctrl-C to stop. |

## Key use patterns

### Post-install health check

```sh
# confirm the daemon came up after install
makakoo daemon status

# expected:
#   running   pid=12345   uptime=2m
```

### Restart after config or plugin changes

```sh
# a clean restart picks up new SANCHO tasks and plugin manifests
makakoo daemon uninstall
makakoo daemon install

# or, on macOS, use launchctl directly for a faster cycle:
launchctl unload ~/Library/LaunchAgents/com.makakoo.daemon.plist
launchctl load  ~/Library/LaunchAgents/com.makakoo.daemon.plist
```

### Debug a failing daemon

```sh
# check the last 100 lines of the log
makakoo daemon logs -l 100

# run in the foreground to see live output
makakoo daemon run
# Ctrl-C to stop
```

## Boot checks & power behavior

- **Fast integrity probe:** On startup (`daemon run` and headless preflight),
  the daemon runs `PRAGMA quick_check(1)` against `superbrain.db` instead of a
  blocking full-database index scan, catching B-tree corruption immediately
  without multi-second startup stalls.
- **Battery-friendly sleep policy:** The idle daemon does **not** hold an OS
  wake lock, so laptops sleep normally when idle. Scoped idle-sleep guards
  (`SleepGuard`) are acquired only during active foreground/maintenance work
  such as `makakoo sync --embed`, `makakoo dream`, `makakoo sancho tick`, and
  opt-in `makakoo run --keep-awake` (disable globally via
  `MAKAKOO_NO_WAKE_LOCK=1`).

## Log location

`$MAKAKOO_HOME/logs/daemon.log` — rotated automatically, max 50 MB.
`makakoo daemon logs` always reads this path regardless of rotation state.

## Related commands

- [`makakoo-sancho.md`](makakoo-sancho.md) — SANCHO runs inside the daemon
- [`makakoo-plugin.md`](makakoo-plugin.md) — plugin changes require a daemon restart
- [`makakoo-status.md`](makakoo-status.md) — top-level system status overview
- [`../troubleshooting/index.md`](../troubleshooting/index.md) — daemon startup errors

## Common gotcha

**`makakoo daemon status` says `not installed` after `makakoo install` on macOS.**
The LaunchAgent plist was written but `launchctl load` was likely blocked by
macOS Background Task management. Go to System Settings → General →
Login Items & Extensions, find Makakoo, and approve it. Then run
`makakoo daemon install` again — it is idempotent and will reload the plist.
Alternatively, log out and back in; the LaunchAgent loads on the next login.
