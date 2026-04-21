---
name: browse
version: 0.1.0
description: |
  Drive a real local Chrome (or Chromium / Edge / Brave) via the Chrome
  DevTools Protocol. Send a Python snippet with `goto`, `click`, `read`,
  `fill`, `screenshot`, `page_info` — get real rendered-DOM results back.
  Use whenever a user asks about a URL, a website, a logged-in page, a
  JavaScript-heavy app, a form, a screenshot, or anything a plain
  HTTP fetch would miss. Powered by upstream browser-use/browser-harness.
allowed-tools:
  - harvey_browse
category: infrastructure
tags:
  - browser
  - chrome
  - cdp
  - web-scraping
  - cli-agnostic
  - mcp-tool
requires:
  - agent-browser-harness plugin installed
  - Chrome started with --remote-debugging-port=9222
  - `makakoo agent start agent-browser-harness` (daemon up)
---

# browse — Chrome CDP driver for every agent

A portable, one-file skill any agentic app can consume. If the runtime
exposes the `harvey_browse` MCP tool (every Makakoo-infected CLI does —
Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen, pi), call
it directly. If not, shell out to `makakoo plugin internal browse` or
point the agent at the upstream harness directly — the Python snippet
contract is the same either way.

## When to reach for it

Call `browse` first when the user:

- Drops a URL and asks *"what's on this page"* / *"read this"* / *"check this site"*.
- Says *"browse to X"* / *"go to X"* / *"scrape X"* / *"visit X"*.
- Asks about behavior requiring JavaScript, login, cookies, or rendered DOM
  that a plain HTTP fetch would miss.
- Wants a screenshot, a DOM query, or to fill a form on a live page.
- Needs to verify a deployment, a UI change, or a running service in the
  browser.

## When NOT to reach for it

- The page is plain public HTML and a plain HTTP fetch handles it — save
  the Chrome round-trip.
- The user asked about a file on disk or an API response (not a browser).
- The user just wants docs / a blog post / a static README — `curl` + the
  LLM's knowledge base is faster.

## Call shape

**MCP runtime** — call the tool directly:

```json
{
  "tool": "harvey_browse",
  "arguments": {
    "code": "goto('https://example.com'); print(page_info())",
    "browser": "default",
    "timeout_s": 60
  }
}
```

Response:

```json
{
  "stdout": "{'title': 'Example Domain', 'url': 'https://example.com', ...}\n",
  "stderr": "",
  "exit_code": 0,
  "browser": "default"
}
```

**Non-MCP runtime (any agent, any host)** — the same Python snippet runs
inside upstream's `run.py`, which reads stdin:

```bash
BU_NAME=default \
  /path/to/.venv/bin/python \
  /path/to/agent-browser-harness/upstream/run.py <<'PY'
goto('https://example.com')
print(page_info())
PY
```

Every `helpers.py` primitive from upstream browser-harness is in scope:
`goto`, `click`, `read`, `fill`, `screenshot`, `page_info`,
`ensure_real_tab`, plus anything declared in `<plugin_dir>/upstream/interaction-skills/`
and `domain-skills/`.

## Multiple browsers

Set `browser: "second"` (or any name) to spawn a second daemon socket.
Useful for A/B testing, parallel session comparison, or driving staging
and prod simultaneously. Each named browser gets its own socket at
`/tmp/bu-<name>.sock` and its own daemon process.

## Prerequisites (one-time)

```bash
# 1. Install the plugin (clones upstream, sets up venv).
makakoo plugin install --core agent-browser-harness

# 2. Start Chrome with CDP exposed.
#    macOS:
open -na "Google Chrome" --args \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-cdp
#    linux:
google-chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp &

# 3. Start the daemon.
makakoo agent start agent-browser-harness

# 4. Restart the CLI so its MCP stdio child re-initializes with
#    harvey_browse registered. (Makakoo MCP children don't hot-reload.)
```

## Error handling contract

If the tool returns:

- **`agent-browser-harness venv python missing at …`** — plugin isn't
  installed. Run step 1 above.
- **`run.py missing at …/upstream/run.py`** — upstream clone didn't
  land. Re-run `makakoo plugin install agent-browser-harness` to
  refresh the clone.
- **Timeout after Ns** — Chrome isn't responding on CDP port 9222, or
  the snippet hung. Verify step 2, then retry with a larger
  `timeout_s`.
- **`chrome: NOT AVAILABLE`** from the daemon doctor — Chrome isn't
  running with `--remote-debugging-port=9222`. Run step 2.

**Do NOT** silently fall back to a plain HTTP fetch when `browse`
fails — the user asked for real browser execution. A fallback is a
confabulation. Surface the error clearly and tell the user which
prerequisite is incomplete.

## Portable install (external agentic apps)

This skill also works outside Makakoo. To wire it into any agent
runtime that supports Python tool calls:

1. `git clone https://github.com/browser-use/browser-harness`
2. `python -m venv .venv && .venv/bin/pip install -e browser-harness/`
3. Expose a tool named `browse` whose implementation calls
   `.venv/bin/python browser-harness/run.py` with the user's Python
   snippet piped via stdin.
4. Set `BU_NAME=<your-agent>` env so the daemon socket is unique.
5. Prereq: user's Chrome must be running with
   `--remote-debugging-port=9222`.

The Python snippet contract is stable across Makakoo + upstream, so a
skill defined against this file works identically in:

- Any MCP-capable CLI with the Makakoo infect applied (Claude Code,
  Gemini CLI, Codex, OpenCode, Mistral Vibe, Cursor, Qwen Code, pi)
- LangChain / LlamaIndex agents (wrap the run.py subprocess)
- Anthropic + OpenAI SDK tool-use loops (expose as a single function)
- Cursor / ChatGPT custom instructions (paste this file as the rule)
- Any terminal-based agent that can shell out

## Attribution

Upstream: [browser-use/browser-harness](https://github.com/browser-use/browser-harness) (MIT).
Makakoo integration: `plugins-core/agent-browser-harness/` in the
makakoo-os repo — `plugin.toml` + `install.sh` + `daemon_admin.py`
(zero vendored upstream code).

## Security notes

- The Makakoo plugin's sandbox profile (`network-io` + scoped filesystem)
  blocks writes outside its own plugin directory. The upstream
  self-healing loop edits `<plugin_dir>/upstream/helpers.py` in place —
  that's allowed; anything else is denied.
- Plugin `[state].retention = "purge_on_uninstall"` — uninstalling wipes
  runtime state (learned selectors, daemon logs). The user's actual
  Chrome profile data lives in their own Chrome user-data-dir, not
  inside the plugin dir, so it survives reinstalls.
- The harness runs arbitrary Python the LLM produces. Audit snippets
  before running them in a privileged context. The standard Makakoo
  capability check still applies — `harvey_browse` won't exceed its
  declared grants (net/http:127.0.0.1, fs/write scoped to plugin dir,
  exec/shell).
