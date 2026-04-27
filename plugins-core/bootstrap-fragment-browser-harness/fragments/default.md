## Local Chrome — always available via `harvey_browse`

Every MCP-capable CLI ships with the `harvey_browse(code, browser?, timeout_s?)` tool when `agent-browser-harness` is installed and started. Use it **whenever a user asks about a website, a URL, a page, or anything that requires real-browser execution** — logged-in sites, JavaScript-heavy apps, PDFs hosted behind auth, forms, anything WebFetch can't reach.

**Trigger patterns** — reach for `harvey_browse` first when:

- The user drops a URL and asks "what's on this page" / "read this" / "check this site" / "open this".
- The user says "browse to X" / "go to X" / "scrape X" / "visit X".
- The user asks about behavior requiring JavaScript, login, cookies, or rendered DOM that WebFetch would miss.
- The user wants a screenshot, a DOM query, or to fill a form on a live page.
- You need to verify a deployment, a UI change, or a running service in the user's browser.

**Don't** reach for `harvey_browse` when:

- The page is plain public HTML and WebFetch can handle it — save the Chrome round-trip.
- You just need docs / a blog post / a static README. WebFetch / a direct download is faster.
- The user asked about a file on disk or an API response (not a browser).

**How to call it** — `harvey_browse` reads a Python snippet from the `code` argument, runs it inside the upstream [browser-harness](https://github.com/browser-use/browser-harness) helpers namespace, returns `{stdout, stderr, exit_code, browser}`:

```python
# Example call payload (LLM-facing JSON input):
{
  "code": "goto('https://example.com'); print(page_info())"
}
# Example output:
# {"stdout": "{'title': 'Example Domain', 'url': 'https://example.com', ...}\n", ...}
```

Every helpers primitive is available inside `code`: `goto`, `click`, `read`, `fill`, `screenshot`, `page_info`, `ensure_real_tab`, plus anything declared in `<plugin_dir>/upstream/interaction-skills/` and `/domain-skills/`.

**Prerequisites (Sebastian, one-time per machine):**

1. `makakoo plugin install --core agent-browser-harness` — clones upstream, sets up venv.
2. Start Chrome with CDP: `google-chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-cdp` (macOS: `open -na "Google Chrome" --args ...`).
3. `makakoo agent start agent-browser-harness` — spawns the daemon.

**If the tool returns "agent-browser-harness venv python missing" or similar**: prereqs aren't met — tell the user exactly which of the three steps is incomplete, do not substitute WebFetch silently (the user asked for real browser execution; a WebFetch fallback is a confabulation).

**Multiple browsers**: set `browser: "second"` (or any name) to spawn a second daemon socket. Useful for A/B testing, parallel session comparison, or driving staging + prod simultaneously.
