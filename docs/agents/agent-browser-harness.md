# `agent-browser-harness`

**Summary:** Browser Use CDP harness — drive local Chrome from any MCP-capable CLI via `harvey_browse`.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-browser-harness/`
**Exposes MCP tool:** `harvey_browse` · **Related walkthrough:** [07 — Browse a website](../walkthroughs/07-browse-website.md)

## When to use

Whenever any infected AI CLI needs to operate a **real rendered browser**: JavaScript-heavy pages, logged-in sites, single-page apps, pages that require cookies, screenshots, form filling, or DOM queries. Plain HTTP fetchers cannot reach these.

**Don't reach for it when** the page is static HTML — a plain `curl` is faster, doesn't start Chrome, and doesn't require CDP.

## Prerequisites

- Chrome (or Chromium / Edge / Brave) installed.
- Chrome must be started with `--remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp` (one-time setup — see walkthrough 07).

## Start / stop

Managed by the Makakoo daemon. Lifecycle toggles via `makakoo plugin`:

```sh
makakoo plugin info agent-browser-harness     # inspect
makakoo plugin disable agent-browser-harness  # turn off
makakoo plugin enable agent-browser-harness   # turn back on
makakoo daemon restart                        # apply (also kicks a stuck daemon)
```

Manual control (advanced — bypass the daemon supervisor):

```sh
cd ~/MAKAKOO/plugins/agent-browser-harness
.venv/bin/python daemon_admin.py start
.venv/bin/python daemon_admin.py health    # "OK" (exit 0) or "DOWN" (exit 1)
.venv/bin/python daemon_admin.py stop
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-browser-harness/`
- **Logs:** `~/MAKAKOO/data/logs/agent-browser-harness.{out,err}.log`
- **Screenshots (from `screenshot()` calls):** `~/MAKAKOO/data/agent-browser-harness/screenshots/`
- **Upstream checkout:** `~/MAKAKOO/plugins/agent-browser-harness/upstream/` (managed by `install.sh` — do not edit by hand).

## Health signals

- `ps -ef | grep daemon_admin.py` — one running Python process.
- `curl -s http://127.0.0.1:9222/json/version` — returns Chrome version JSON (confirms CDP is reachable).
- `makakoo plugin info agent-browser-harness` — `enabled: yes`.
- `.venv/bin/python daemon_admin.py health` — prints `OK`, exits 0.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `Chrome CDP not reachable at 127.0.0.1:9222` | Chrome not running with `--remote-debugging-port=9222` | Close Chrome fully (Cmd+Q), restart with the flags from walkthrough 07 step 2. |
| `agent-browser-harness venv python missing` | `install.sh` didn't finish — no venv | `makakoo plugin install --core agent-browser-harness` to re-run install.sh. |
| `harness returned error: session closed` | Chrome was closed or the profile lock conflicts | Kill all Chrome processes, restart with the walkthrough-07 flags. |
| `harvey_browse` not visible in the CLI's MCP tool list | CLI started before daemon spawned the harness | Restart the CLI. Confirm with `makakoo infect --verify`. |

## Capability surface

Declared in `plugin.toml`:

- `net/http:127.0.0.1` — talks to Chrome's CDP port.
- `net/http:api.browser-use.com` — upstream harness telemetry (opt-out via upstream config).
- `fs/read:$MAKAKOO_HOME/plugins/agent-browser-harness`
- `fs/write:$MAKAKOO_HOME/plugins/agent-browser-harness`
- `exec/shell`

Nothing else. The harness cannot touch the rest of your filesystem, cannot reach arbitrary hosts, cannot exec outside its venv.

## Remove permanently

```sh
makakoo plugin uninstall agent-browser-harness --purge
```

`--purge` also deletes `~/MAKAKOO/state/agent-browser-harness/` and screenshots. Omit it to keep the state for a future reinstall.
