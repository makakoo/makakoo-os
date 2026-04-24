# Walkthrough 07 — Open and read a website with Harvey

## What you'll do

Point Harvey at a live URL and have him actually drive a real Chrome browser to read the page — including JavaScript-rendered content, logged-in sessions, and anything a plain HTTP fetch would miss. This uses the `harvey_browse` MCP tool provided by the `agent-browser-harness` plugin.

**Time:** about 6 minutes (most of it one-time Chrome setup). **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), an MCP-capable AI CLI installed and infected (see [Walkthrough 05 — Ask Harvey, Path 1](./05-ask-harvey.md)).

## Why this exists

`curl` and plain HTTP fetches are blind to modern web apps: anything rendered by JavaScript, anything behind login, anything that needs a cookie, anything that is technically a "single-page app" — they all return near-empty HTML to plain fetchers. To read what a human actually sees, you need a real browser.

`harvey_browse` gives every Makakoo-infected AI CLI a real Chrome to drive, via the **Chrome DevTools Protocol (CDP)** — the same protocol the Chrome DevTools panel uses to inspect a page. You don't write Selenium scripts; you describe what you want in natural language, and your AI CLI emits a tiny Python snippet (using `goto`, `click`, `read`, `fill`, `screenshot`, `page_info`) that the harness executes.

## Steps

### 1. Install the browser-harness plugin (if it's not already)

The `core` distro includes `agent-browser-harness` pre-installed. Confirm:

```sh
makakoo plugin info agent-browser-harness
```

If the output starts with `error: plugin not installed`, install it:

```sh
cd ~/makakoo-os
makakoo plugin install --core agent-browser-harness
```

The plugin's `install.sh` clones the upstream [browser-use/browser-harness](https://github.com/browser-use/browser-harness) repo into its own `.venv` — one-time, about 30 seconds.

### 2. Start Chrome with the DevTools Protocol port open

Close any running Chrome instance first (Chrome only allows one CDP-enabled session at a time with a given user-data-dir).

Then start Chrome with two extra flags:

```sh
open -na "Google Chrome" --args \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-cdp
```

- `--remote-debugging-port=9222` — this is the port the harness connects to.
- `--user-data-dir=/tmp/chrome-cdp` — a throwaway profile so your daily browsing doesn't interfere.

A new Chrome window opens with the `/tmp/chrome-cdp` profile. You can navigate normally in it; `harvey_browse` shares its state.

> **Linux / Windows:** replace `open -na "Google Chrome"` with `google-chrome` (Linux) or the full path to `chrome.exe` (Windows). Same flags.

### 3. Confirm the daemon is running

```sh
makakoo daemon status
```

Expected output:

```text
makakoo daemon: running
log dir: /Users/you/MAKAKOO/data/logs
```

The daemon is what actually spawns the `agent-browser-harness` background process. If the output says `not running`, run `makakoo daemon install` + let it start.

### 4. Verify the harness daemon is alive

```sh
ps aux | grep -v grep | grep browser-harness
```

Expected output (abbreviated, your PID and paths will differ):

```text
sebastian   95364  Python /Users/you/MAKAKOO/plugins/agent-browser-harness/upstream/daemon.py
```

If no match, force a restart:

```sh
makakoo daemon restart
```

If you need to drive the agent directly (bypass the daemon), use `makakoo agent {start,stop,status,health} agent-browser-harness` — a thin wrapper around the plugin's declared `[entrypoint]` scripts. The daemon remains the primary supervisor; this subcommand is the escape hatch for manual control.

### 5. Open your AI CLI and ask Harvey to browse a page

Open Claude Code (or any infected CLI):

```sh
claude
```

At the prompt, try:

```text
Use harvey_browse to go to https://news.ycombinator.com and give me the top 3 story titles.
```

**What happens:**
1. The AI CLI recognizes the phrase "harvey_browse" / "go to https://" as a trigger for the MCP tool.
2. It composes a short Python snippet, e.g.:
   ```python
   goto("https://news.ycombinator.com")
   info = page_info()
   titles = [a.text for a in read(".titleline > a")[:3]]
   print(titles)
   ```
3. The snippet goes to the harness, which runs it against your live Chrome.
4. The return value comes back to the AI CLI, which answers you in plain language citing the 3 titles.

You should see the AI reply with 3 actual current-day HN story titles, plus the URLs.

### 6. Ask for a screenshot

```text
Screenshot the top of https://example.com and tell me the font of the headline.
```

The harness saves the screenshot under `~/MAKAKOO/data/agent-browser-harness/screenshots/` and the AI CLI describes the image — powered by the same multimodal path as `harvey_describe_image`.

### 7. See what the plugin actually logged

```sh
makakoo daemon logs --plugin agent-browser-harness 2>/dev/null | tail -20
```

(If the `--plugin` flag isn't wired on your version, fall back to `makakoo daemon logs | grep browser-harness | tail -20`.)

You'll see one line per harness invocation, including the snippet it ran.

## What just happened?

- **The harness is not a mock.** It's your real Chrome, running in a real window, with a real DOM. Click-tracking, cookies, localStorage, service workers — everything works. Drive it from the AI CLI, and you see the results in the Chrome window in real time.
- **No Selenium, no Puppeteer in your code.** The AI CLI emits a small Python snippet using the browser-harness helpers vocabulary. The harness interprets the snippet; you never wrote browser-automation code.
- **`harvey_browse` is capability-gated.** The plugin declares `net/http:127.0.0.1,api.browser-use.com` + `exec/shell` in its manifest — that's the entire blast radius. It cannot touch your filesystem outside its own plugin directory, cannot talk to arbitrary network endpoints, and cannot exec anything outside its venv.
- **One browser, many agents.** Set the `browser` argument on a `harvey_browse` call to spawn a second daemon socket (`"second"`, `"prod"`, any name). Useful for A/B testing a staging site against prod simultaneously.

## If something went wrong

| Symptom | Fix |
|---|---|
| `agent-browser-harness venv python missing` | The plugin's install.sh didn't finish — rerun `makakoo plugin install --core agent-browser-harness`. |
| `Chrome CDP not reachable at 127.0.0.1:9222` | Chrome isn't running with `--remote-debugging-port=9222`. Close Chrome fully (Cmd+Q), restart with the flags from step 2. |
| `harness returned error: session closed` | Chrome was closed or the profile lock conflicts. Restart Chrome with the step-2 command. |
| AI CLI doesn't seem to know what `harvey_browse` is | The CLI hasn't picked up the MCP tool list. Exit and reopen the CLI. Also confirm `makakoo infect --verify` shows `clean` for that CLI. |
| Harness fails on a logged-in page | The `/tmp/chrome-cdp` profile is fresh each time. Log into the service manually in that Chrome window once, then your cookies persist across harness calls until you delete `/tmp/chrome-cdp`. |

## Next

- [Walkthrough 08 — Use an agent](./08-use-agent.md) — the broader agents-as-plugins lifecycle (browser-harness is one of ~11 agents shipped).
- [Walkthrough 09 — Ingest a document](./09-ingest-document.md) — pull a PDF or a YouTube URL into the knowledge index; `harvey_browse` is how you feed URL-hosted documents in.
