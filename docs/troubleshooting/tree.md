# Troubleshooting — symptom tree

Use this page when something is wrong and you want a fix, not an explanation. Start with the top-level symptom that matches what you observed. Each branch narrows down to a concrete fix — one command to run, one file to edit, or one walkthrough to re-read.

If nothing on this page matches, the flat reference is at [`index.md`](./index.md) and the error-string index is at [`symptoms.md`](./symptoms.md).

---

## Top-level symptoms

1. [I ran a command and got an error](#i-ran-a-command-and-got-an-error)
2. [The command ran but nothing happened](#the-command-ran-but-nothing-happened)
3. [It worked yesterday, not today](#it-worked-yesterday-not-today)
4. [I don't know what command to run](#i-dont-know-what-command-to-run)
5. [Harvey / MCP not responding](#harvey--mcp-not-responding)
6. [Plugin install failed](#plugin-install-failed)
7. [Octopus peer unreachable](#octopus-peer-unreachable)
8. [Docs MCP not working](#docs-mcp-not-working)

---

## I ran a command and got an error

### `makakoo: command not found`

`makakoo` is not on your `$PATH`. Two causes:

- **Installed via `curl | sh` but didn't reload the shell** → `exec $SHELL`.
- **Installer placed the binary in `~/.local/bin/` which isn't on `$PATH`** → add `export PATH=$HOME/.local/bin:$PATH` to `~/.zshrc`, reload.

Verify: `which makakoo` should return a path.

### `error: unrecognized subcommand '<name>'`

You typed a subcommand that doesn't exist. Either:

- **Typo** → `makakoo --help` to see the real list.
- **You're thinking of a subcommand from an older version** → `makakoo --version`. If below `0.1.0`, upgrade.
- **You're thinking of a subcommand that the v1 sprint draft mentioned but was never implemented** (e.g. `makakoo doctor`, `makakoo agent start`) → see DOGFOOD-FINDINGS in the grandma-docs sprint workspace.

### `error: plugin not installed: <name>`

Either misspelled or not installed yet. Confirm spelling:

```sh
makakoo plugin list | grep <partial>
```

If the plugin you want isn't there, install it:

```sh
cd ~/makakoo-os && makakoo plugin install --core <full-name>
```

### `error: staging error: target plugin dir already exists — uninstall first`

You're trying to install a plugin that's already present. Either uninstall first or use `makakoo plugin update <name>` to refresh from the source.

### `error: too broad: '/'` (or `~`, `$HOME`, `*`, `**`)

You passed a path to `makakoo perms grant` that's too permissive. Pick a specific subdirectory:

```sh
makakoo perms grant ~/Projects/my-app/ --for 1h
```

### `error: rate limit`

Too many `perms grant` calls in the last hour (limit: 50/hour, 20 active grants). Either revoke some:

```sh
makakoo perms list
makakoo perms revoke <id>
```

or wait the window out.

### `error: llm error: http 400: unknown provider for model <alias>`

The model alias in your request isn't registered with your configured LLM gateway. Two fixes:

- **Change model** → `makakoo query --model <alias-your-provider-uses> "..."`.
- **Register the alias** → edit your gateway config (for `switchAILocal`, it's `~/.switchailocal/config.json`).

Reported as DOGFOOD-FINDINGS F-004.

### `error: load/create signing key: <os-error>`

Octopus bootstrap couldn't write `~/MAKAKOO/config/peers/signing.{key,pub}`. Usually permissions. Check:

```sh
ls -la ~/MAKAKOO/config/peers/
```

The directory should be `chmod 700` and the key file `chmod 600`. Fix with:

```sh
mkdir -p ~/MAKAKOO/config/peers
chmod 700 ~/MAKAKOO/config/peers
```

---

## The command ran but nothing happened

### `makakoo sync` reports `0 new` after you wrote a journal line

The file didn't save, or you edited the wrong file. Verify:

```sh
grep "your line" ~/MAKAKOO/data/Brain/journals/*.md
```

If empty: re-open the editor, confirm the line starts with `- `, save again, rerun `makakoo sync`.

### `makakoo search` returns no results for content you know is in the Brain

The index drifted from the files. Force a full rebuild:

```sh
makakoo sync --force
```

### `makakoo sancho tick` runs but no mascot breadcrumbs appear

`mascot-gym` isn't installed or disabled:

```sh
makakoo plugin list | grep mascot-gym
# if missing: install from --core; if disabled: enable then daemon restart
```

### Infected CLI doesn't acknowledge the Makakoo bootstrap block

The CLI's global instructions file was edited after infect ran, or the CLI was open before infect. Run:

```sh
makakoo infect --verify       # check drift
makakoo infect                # rewrite if drifted
# then close and reopen the CLI
```

---

## It worked yesterday, not today

### Daemon stopped overnight

macOS occasionally kills LaunchAgents after a sleep / wake cycle. Check:

```sh
makakoo daemon status
makakoo daemon restart      # or `makakoo daemon install` if it says not installed
```

### Plugin broke after a `makakoo` upgrade

A schema migration may have reset `plugins.lock`. Run:

```sh
makakoo plugin sync        # re-registers plugins-core/ into the live tree
makakoo daemon restart
```

### Brain searches return stale results

The FTS5 index wasn't rebuilt after a file edit. Force:

```sh
makakoo sync --force
```

### Octopus calls 401-signature-invalid after working yesterday

Clock drift > 60s between peers, or a peer's key rotated. Fix clock first (NTP):

```sh
sudo sntp -sS time.apple.com   # macOS
```

Then:

```sh
makakoo octopus doctor
```

If drift wasn't the cause, re-run `makakoo octopus join` from the peer side.

---

## I don't know what command to run

### Goal: see what I have installed

```sh
makakoo plugin list
makakoo nursery list                 # mascots
makakoo sancho status                # scheduled tasks
makakoo octopus trust list           # peer grants (if any)
```

### Goal: see what any command does before running it

```sh
makakoo --help                       # top-level tree
makakoo <subcommand> --help
makakoo setup --non-interactive      # show current config without prompting
```

### Goal: ask the Brain something

If you have a configured AI CLI (Claude Code, Gemini, …): just open the CLI and ask. The bootstrap block (from `makakoo infect`) makes it Brain-aware.

If you don't have one or prefer a direct pipe: `makakoo query "<question>"`.

### Goal: "something is broken and I don't know what"

Three health-check commands:

```sh
makakoo sancho status        # task engine alive?
makakoo memory stats         # memory layer responding?
makakoo octopus doctor       # octopus peer layer healthy?
```

If all three are green, it's not infrastructure — the bug is narrower. Isolate by feature.

---

## Harvey / MCP not responding

### The `makakoo-mcp` process isn't showing up in the CLI's tool list

```sh
ps -ef | grep makakoo-mcp
```

If no match, run `makakoo daemon restart`. If the process runs but the CLI can't see tools, close and reopen the CLI.

### The process is running but tool calls hang

The stdio worker pool may be deadlocked. Restart:

```sh
makakoo daemon restart
```

Consistent hangs under load on macOS + WireGuard are a known interaction (tokio/mio kqueue on utun); the Python shim at `~/MAKAKOO/plugins/lib-harvey-core/src/core/mcp/http_shim.py` is the workaround.

### A specific tool returns `resource exhausted` or `rate limit`

The upstream LLM hit a cap. The MCP wrapper retries with exponential backoff; if it still fails after retries, wait or switch models:

```sh
makakoo query --model <alternate-alias> "..."
```

**Never** journal a URL as a workaround for a failed describe (F-004 anti-pattern).

---

## Plugin install failed

### `error: resolve plugins-core: can't find plugins-core/`

You ran `makakoo plugin install --core <name>` outside a repo checkout. Fix:

```sh
cd ~/makakoo-os && makakoo plugin install --core <name>
```

Or set `MAKAKOO_PLUGINS_CORE=/absolute/path/to/plugins-core` in your environment.

### `blake3 mismatch`

The plugin source on disk doesn't match the pinned hash. Either the source was tampered with or you pulled a newer version. To accept the new hash:

```sh
makakoo plugin install --core <name> --blake3 <new-hash-from-error-msg>
```

### `sha256 required for tarball sources`

You passed an `https://...tar.gz` URL without a `--sha256` flag. This is required for remote tarballs to prevent silent supply-chain swaps.

```sh
makakoo plugin install https://... --sha256 <expected-hash>
```

### `install.sh exited non-zero`

The plugin's install script failed. Find the actual cause:

```sh
tail -50 ~/MAKAKOO/data/logs/plugin-install-<name>.log
```

The error is almost always a missing system dep (git, curl, Python, Node) — install it and rerun.

### Warning spam: `skipping plugin — manifest failed to parse`

The plugin has a malformed `plugin.toml` (the canonical example is `language = "markdown"` — DOGFOOD-FINDINGS F-006). The rest of your plugins are unaffected. To fix the specific plugin, edit its manifest.

---

## Octopus peer unreachable

### `makakoo octopus doctor` reports `trust store: out of sync`

The shim trust file drifted from the JSON store. Fix:

```sh
makakoo octopus trust list
# for any stale peer:
makakoo octopus trust revoke <peer-name>
# then re-invite / re-join
```

### `makakoo octopus join` errors with `invite expired`

Default invite duration is 1h. Ask the inviting host to generate a new invite with `--duration 24h`.

### `makakoo octopus join` hangs with no output

Two common causes:

- **Firewall** blocks outbound to the host on port 8765 → open it, or tunnel through WireGuard (Tytus pods already do this).
- **Host's shim not listening** → on the host side, `makakoo octopus doctor` should show a `lsof` line proving the port is bound. If not, restart the `agent-octopus-peer` plugin.

### Peer call returns `429 rate limited`

You exceeded the default 30 writes/min per peer. Slow down, or ask the granting host to re-issue the grant with a higher rate.

### Peer call returns `401 signature invalid`

Clock drift > 60s, or peer's pubkey changed. Sync clocks (`sntp`), verify pubkeys match on both sides (`makakoo octopus trust list`), re-join if mismatched.

---

## If nothing here matches

- Verbatim-error-string index: [`symptoms.md`](./symptoms.md).
- Flat prose: [`index.md`](./index.md).
- Uninstall / clean reinstall: [`uninstall.md`](./uninstall.md).
- File an issue with: `makakoo version` output + the exact command + the exact error + one line of what you expected.

---

## Docs MCP not working

The bundled `makakoo docs-mcp --stdio` MCP server lets AI CLIs query Makakoo's public docs in real time. Setup guide: [`../docs-mcp-setup.md`](../docs-mcp-setup.md). If something's off:

### MCP server not appearing in your AI CLI

Most common cause: `makakoo` isn't on the `PATH` *of the process that spawns the MCP server*. Some CLIs (Claude Code, Cursor) launch from GUI shells with a different `PATH` than your terminal.

- Verify from a terminal: `which makakoo` returns a path.
- If your CLI was launched from the GUI: use the absolute path in the MCP config — `"command": "/Users/you/.local/bin/makakoo"` instead of `"command": "makakoo"`.
- Restart the CLI fully (not just reload window) after editing its MCP config.

### Search returns nothing

The docs corpus is baked into the binary at compile time. If your binary is old, the corpus is old. Two fixes:

- **Refresh the cache:** `makakoo docs update --from-github` pulls the latest from `main` and writes to `~/.makakoo/docs-cache/index.db`. The MCP server prefers the cache over the baked corpus on next start.
- **Upgrade the binary:** `brew upgrade makakoo` (or re-run the install script). Each release bakes a fresh corpus.

### `initialize failed` in the CLI's MCP logs

Almost always means `--stdio` is missing from `args`. The server exits with usage help if it sees no `--stdio` flag. Check the config:

```json
{ "makakoo-docs": { "command": "makakoo", "args": ["docs-mcp", "--stdio"] } }
```

The `["docs-mcp", "--stdio"]` array order matters — `--stdio` must be present.

### Citation `path` field looks wrong

The `path` field returned by `makakoo_docs_search` / `read` / `list` / `topic` is **repo-relative**, e.g. `docs/concepts/architecture.md`. To open it:

- In the browser: `https://github.com/makakoo/makakoo-os/blob/main/<path>`
- Locally: `git clone https://github.com/makakoo/makakoo-os && less <path>` from the repo root.

If your CLI doesn't render the citation as a link, that's an MCP-host UI issue, not a docs-MCP bug — the data is correct.

### `version too old` or schema mismatch on cache load

The cache file at `~/.makakoo/docs-cache/index.db` is version-gated by `meta.built_for_version`. After a `makakoo` upgrade where the FTS5 schema or tokenizer changed, the cache silently falls back to baked corpus until you re-run:

```sh
makakoo docs update --from-github
```

To force-clear the cache: `rm -rf ~/.makakoo/docs-cache/`. Next MCP server start will use baked corpus until you refresh.
