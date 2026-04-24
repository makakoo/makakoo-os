# Walkthrough 06 — Grant write access to a specific folder

## What you'll do

Ask Makakoo for permission to write to a folder outside its default sandbox, watch it grant a **time-limited** write permission, and revoke it when you're done. You'll also see the audit log that records every grant and revoke.

**Time:** about 3 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md).

## Why this exists

By default, any `write_file` call — whether from Makakoo itself or from an AI CLI using Makakoo's MCP tools — is **sandboxed**. It can only write to a small list of safe folders:

- `~/MAKAKOO/data/reports/`
- `~/MAKAKOO/data/drafts/`
- `~/MAKAKOO/tmp/`
- `/tmp/`

If Harvey tries to write anywhere else — e.g. into your `~/Documents/`, `~/Projects/my-app/`, `~/Desktop/` — the write is **rejected** with a clear error. This is a safety feature: it means a bug in an AI agent can't silently clobber your code or your important files.

When you do want Harvey to write somewhere else, you grant a permission. Grants are:

- **Explicit** — you asked for it, nothing is implicit.
- **Time-limited** — default is 1 hour. You can pick 30m, 1h, 24h, 7d, or `permanent` (only for paths under `$MAKAKOO_HOME`, or with a `--yes-really` override).
- **Scope-safe** — `/`, `~`, `$HOME`, `*`, `**` are rejected at the handler no matter who asks.
- **Revocable** — one command takes it back.
- **Audited** — every grant, revoke, and actual write against a grant is logged.

## Steps

### 1. See what grants exist right now

```sh
makakoo perms list
```

On a fresh install:

```text
(no grants)
```

No active grants — baseline sandbox only. Harvey can only write to the four paths listed above.

### 2. Ask for a 1-hour grant

Let's say you want Harvey to be able to write drafts into `/tmp/makakoo-grandma-test/`:

```sh
makakoo perms grant /tmp/makakoo-grandma-test/ --mkdir --label "walkthrough-06-demo"
```

- `--mkdir` creates the target folder if it doesn't exist yet (otherwise you'd see `target does not exist — pass --mkdir to create it`).
- `--label` is a short free-text note that shows up in `perms list` — useful when you have several grants at once and want to remember why each one is there.

Expected output:

```text
Granted g_20260424_8b3ee8f5. /tmp/makakoo-grandma-test/ writable until 15:32 UTC (1h). Revoke: makakoo perms revoke g_20260424_8b3ee8f5
```

The grant ID (`g_<date>_<random>`) is **yours forever** — Makakoo will print it back at you whenever you list grants, and you'll use it to revoke.

### 3. Confirm it's active

```sh
makakoo perms list
```

Expected output:

```text
ID                      EXPIRES           SCOPE                                          LABEL
g_20260424_8b3ee8f5     in 0h59m          fs/write:/tmp/makakoo-grandma-test/**          walkthrough-06-demo
```

The `**` at the end of the scope means "this folder and anything inside it recursively".

### 4. Use it (optional demonstration)

Any write into `/tmp/makakoo-grandma-test/` — from Harvey, from an infected CLI, from a direct MCP call — is now allowed. Example:

```sh
echo "Harvey wrote this." > /tmp/makakoo-grandma-test/hello.txt
```

(That command uses plain shell, which doesn't go through Makakoo's sandbox — but it demonstrates the folder is ready. If an AI CLI calls `write_file("/tmp/makakoo-grandma-test/hello.txt", "...")`, the sandbox now allows it.)

### 5. Revoke when you're done

```sh
makakoo perms revoke g_20260424_8b3ee8f5
```

Replace `g_20260424_8b3ee8f5` with the actual grant ID you got. Expected output:

```text
Revoked g_20260424_8b3ee8f5.
```

### 6. Confirm it's gone

```sh
makakoo perms list
```

Expected output:

```text
(no grants)
```

Back to baseline. Harvey can no longer write to `/tmp/makakoo-grandma-test/`.

### 7. Inspect the audit log

Every grant, revoke, and failed write gets an audit entry:

```sh
makakoo perms audit --since 1h
```

Expected output (truncated — one entry per action in the last hour):

```text
2026-04-24T15:32:03Z   perms/grant    g_20260424_8b3ee8f5  /tmp/makakoo-grandma-test/**   1h   label=walkthrough-06-demo
2026-04-24T15:33:11Z   perms/revoke   g_20260424_8b3ee8f5
```

## What just happened?

- The **three-layer capability model**: baseline (compile-time) + manifest grants (declared by plugins) + runtime grants (what you just did) all combine to decide if a write is allowed. Runtime grants are the only layer you edit without changing code or restarting anything.
- **Grants are stored in `~/MAKAKOO/state/perms/grants.json`** and read on every `write_file` call. There's no daemon to restart.
- **Expiry is enforced server-side**, not on the clock of the calling program. A 1-hour grant expires in the handler's database regardless of what the CLI thinks.
- The flow you just ran is what an AI CLI can do **through you**: when an AI tool call hits a `write_file rejected` error, the CLI is supposed to stop, offer you the grant (with the suggested duration), wait for your explicit "yes", then call `perms grant` on your behalf. You always have the final word.

## If something went wrong

| Symptom | Fix |
|---|---|
| `error: target <path> does not exist — pass --mkdir to create it` | Add `--mkdir` if you want the grant to also create the folder. |
| `error: too broad: '~'` (or `/`, `*`, `**`, `$HOME`) | Those scopes are hard-refused — pick a specific subfolder. |
| `error: rate limit` | You've hit the global rate limit (20 active grants or 50 create-ops/hour). Revoke some, or wait. |
| A grant looks right in `perms list` but `write_file` still rejects | Check the exact path in the error. Grants are path-prefix matched; if you granted `/tmp/foo/` and the write is to `/tmp/foo`, it works, but a write to `/tmp/foobar/` does NOT (no path-traversal). |
| `perms audit` is empty even after a grant | The audit log might not exist yet — first grant creates it. Run `makakoo perms grant` once, then audit. |

## Next

- [Walkthrough 07 — Browse a website with Harvey](./07-browse-website.md) — use `harvey_browse` to make Makakoo operate a real Chrome instance.
- [Walkthrough 09 — Ingest a document](./09-ingest-document.md) — pull a PDF into the knowledge index (may require a grant if the PDF is outside the sandbox).
