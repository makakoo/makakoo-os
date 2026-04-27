# `agent-browser-harness` — Chrome CDP driver for every CLI

`agent-browser-harness` is the flagship git-sourced plugin that ships with v0.4. It wraps [browser-use/browser-harness](https://github.com/browser-use/browser-harness) so every MCP-capable CLI (Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen) can drive your local Chrome through the Chrome DevTools Protocol.

Upstream code is NOT vendored. The wrapper is ~150 lines (`plugin.toml` + `install.sh` + `daemon_admin.py`). `install.sh` shallow-clones `browser-use/browser-harness` at install time; `makakoo plugin update agent-browser-harness` refetches.

## TL;DR

```bash
# 1. Install the plugin (clones upstream, sets up venv, pip install -e .)
#    Upstream requires Python >=3.11 — on macOS the default python3 is
#    3.9, so override via MAKAKOO_VENV_PYTHON.
MAKAKOO_VENV_PYTHON=python3.13 makakoo plugin install --core agent-browser-harness

# 2. Start your local Chrome with CDP exposed
open -na "Google Chrome" --args \
  --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp   # macOS
# OR
google-chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp &  # Linux

# 3. Start the agent daemon
makakoo agent start agent-browser-harness

# 4. Restart your CLI — MCP stdio children don't hot-reload. The new
#    session will see harvey_browse in its tool list.

# 5. From any MCP-capable CLI (Claude Code, Gemini, Codex, OpenCode,
#    Vibe, Cursor, Qwen, pi):
harvey_browse code="goto('https://example.com'); print(page_info())"
```

## Python version prereq (`MAKAKOO_VENV_PYTHON`)

Upstream `browser-harness` declares `python = ">=3.11"` in its
`pyproject.toml`. Without an override, the plugin's venv is built with
whatever `python3` is first on PATH — which is **Python 3.9** on
stock macOS — and pip rejects the install with:

```
ERROR: Package 'harness' requires a different Python: 3.9.6 not in '>=3.11'
```

Fix: set `MAKAKOO_VENV_PYTHON=python3.11` (or `.12` / `.13`) for the
install invocation. Homebrew's `brew install python@3.13` is the
fastest path to an interpreter on macOS; `pyenv` works too.

Once installed, the venv remembers its interpreter — future runs don't
need the env var.

## What's in the wrapper

- `plugins-core/agent-browser-harness/plugin.toml` — kind=agent, sandbox profile `network-io`, MCP tool `harvey_browse`.
- `install.sh` — shallow-clones `https://github.com/browser-use/browser-harness` into `<plugin_dir>/upstream/`, bootstraps `.venv`, pip-installs editable.
- `daemon_admin.py` — CLI shim that routes `makakoo agent start/stop/health agent-browser-harness` into upstream's `admin.py` primitives.

Env overrides:

| Variable | Default | Effect |
|---|---|---|
| `BROWSER_HARNESS_UPSTREAM` | `https://github.com/browser-use/browser-harness` | Swap the upstream fork |
| `BROWSER_HARNESS_REF` | `main` | Pin to a tag or SHA (fork maintainer convention) |
| `BU_NAME` | `default` | Name the daemon socket so multiple browsers can coexist |
| `BU_CDP_URL` | `http://127.0.0.1:9222/json/version` | Doctor probe URL |

## First browse

```python
# harvey_browse reads Python from stdin via upstream's run.py, so the
# full helpers API is available. Tip: start with page_info() to confirm
# the harness is connected.
goto("https://example.com")
info = page_info()
print(info)
```

The `harvey_browse` MCP tool returns:

```json
{
  "stdout": "{'title': 'Example Domain', ...}\n",
  "stderr": "",
  "exit_code": 0,
  "browser": "default"
}
```

## Chrome setup

browser-harness talks to Chrome over CDP. Start Chrome (or Edge / Chromium / Brave) with the remote debugging port exposed:

```bash
# macOS
open -na "Google Chrome" --args \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-cdp

# Linux
google-chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp &
```

`makakoo agent health agent-browser-harness` pings the socket; `daemon_admin.py doctor` pings CDP. Both must be green for `harvey_browse` to work.

## Authoring domain-skills

Upstream supports markdown-only "domain-skills" — one file per site with an LLM-friendly recipe (selectors, flows, gotchas). Those live under `<plugin_dir>/upstream/domain-skills/` post-install. Add yours there or upstream them via PR; the self-healing loop edits `helpers.py` at runtime, so you never touch Python unless you want to.

## Remote browsers (optional)

`browser-harness` also supports remote browsers via Browser Use Cloud. Set:

```bash
export BROWSER_USE_API_KEY=bu_live_…
```

Then:

```python
admin.start_remote_daemon(name="cloud-1", profileName="work")
```

No changes to the Makakoo plugin — the daemon just connects to a remote Chrome instead of your local one.

## Updating

```bash
makakoo plugin outdated      # any drift?
makakoo plugin update agent-browser-harness  # refetch, re-prompt on capability drift, reinstall
```

If upstream changes its manifest (`pyproject.toml` deps, CLI entrypoints, etc.), Makakoo re-prompts for re-trust before promoting the new tree. Decline → installed version stays put; accept → plugin restarts and picks up the new code.

## Security notes

- The plugin's sandbox profile (`network-io` + scoped filesystem) blocks writes outside its own plugin dir. The self-healing loop edits `<plugin_dir>/upstream/helpers.py` in place — that's allowed; anything else outside `$MAKAKOO_HOME/plugins/agent-browser-harness/` is denied.
- `state.retention = "purge_on_uninstall"` — uninstalling wipes the plugin's runtime state (learned selectors, daemon logs). Your Chrome profile data lives in your own Chrome user dir, not here.
- OpenClaw, Counsel, and other external brands are not implicated here — `browser-harness` is a standalone tool maintained upstream. Attribution: Browser Use (upstream) + Makakoo OS (integration glue only).

## When this plugin is not enough

- **Headless / CI**: this harness wants a real Chrome with CDP. For headless browse-and-extract in CI, use Playwright directly.
- **Cross-domain session replay**: browser-harness is ephemeral per-call; for multi-turn logged-in workflows across sites, pair it with a remote Chrome and persistent user-data-dir.
