# Symptoms — verbatim error-string index

Every error string the `makakoo` binary (or one of its Rust subsystems) can emit, mapped to the section in [`tree.md`](./tree.md) that has the fix.

Search this page (`Ctrl+F` / `⌘+F`) for the exact wording you saw. If your symptom isn't here, the tree's **categories** are still organized by observable symptom and usually have a hit.

---

## A

- **`--http must be ADDR:PORT or :PORT`** — [Plugin install failed → invalid daemon flag](./tree.md#plugin-install-failed). Or: you passed `--http` without a valid listen spec to the MCP server; pass `:8765` or `0.0.0.0:8765`.
- **`ambiguous path — <N> grants match <path>`** — You called `makakoo perms revoke --path <p>` and multiple grants' scopes cover the same path. Pick an id: `makakoo perms list` → `makakoo perms revoke <g_id>`.
- **`SKILL.md declares entry: '<path>' but file does not exist in <dir>`** — Plugin `SKILL.md` frontmatter names an `entry:` file that isn't present. Either add the file or remove the declaration. Ziggy (mascot) surfaces this class of issue — see [ziggy.md](../mascots/ziggy.md).

## B

- **`blake3 mismatch`** — [Plugin install failed → blake3 mismatch](./tree.md#plugin-install-failed).

## C

- **`can't find distros/` / `can't find plugins-core/`** — You ran a `--core` / `--distro` command from outside a repo checkout. [Plugin install failed → resolve plugins-core](./tree.md#plugin-install-failed). Set the appropriate env var (`MAKAKOO_PLUGINS_CORE=<path>` or `MAKAKOO_DISTROS=<path>`) or `cd` into the checkout.
- **`cannot locate lib-harvey-core/src/`** — Makakoo needs `lib-harvey-core` to resolve Python mascot/agent imports. Install with `makakoo plugin install --core lib-harvey-core`, or set `MAKAKOO_PLUGINS_DIR` to a source-tree `plugins-core/` directory.
- **`cannot resolve $HOME`** — Your environment has no `$HOME` variable set. This is deeply abnormal — check `echo $HOME`. On macOS/Linux it's always set; on Windows (WSL) it may not be. Set `HOME=/Users/<you>` before running `makakoo`.
- **`command not found: makakoo`** — [I ran a command and got an error → `makakoo: command not found`](./tree.md#makakoo-command-not-found).
- **`Continue config is not a JSON object`** — An IDE integration hit a malformed `~/.continue/config.json`. Restore or regenerate the file; `makakoo infect` will re-populate once the JSON is valid.

## D

- **`duration <value> exceeds 365 days — shorten or split into multiple grants`** — `makakoo perms grant --for <duration>` accepts up to 365 days. Split into multiple grants if you truly need longer, or use `--for permanent` (requires `--yes-really` outside `$MAKAKOO_HOME`).

## E

- **`empty command`** — An adapter manifest declared a blank `[entrypoint]` command. Edit the plugin's `plugin.toml` or reinstall the plugin from source.
- **`empty duration; use one of: 30m, 1h, 24h, 7d, permanent`** — `makakoo perms grant --for ""`. Pass one of the listed values.
- **`empty scope — grant a specific directory`** — [I ran a command and got an error → `error: too broad`](./tree.md#error-too-broad----or-~-home---).
- **`expanded scope resolves to root — refuse to grant filesystem-wide write`** — The path you passed expanded to `/`. Pick a specific subdirectory.

## F

- **`failed to read <path>: <os-error>`** — Filesystem permission or missing-file. Check `ls -la <path>`.
- **`failed to spawn plugin '<name>': <error>`** — The daemon tried to start a plugin's entrypoint and the process launch failed. Check the plugin's entrypoint in `plugin.toml` vs what's actually on disk; Cinder (mascot) auto-surfaces compile-time issues — see [cinder.md](../mascots/cinder.md).
- **`failed to write bootstrap cache: <error>`** — The infect cache path isn't writable. `~/MAKAKOO/cache/infect/` must be writable by your user.

## G

- **`GET <url>: <err>`** — Network failure during an HTTP GET (plugin install from tarball, harvey_browse download). Check connectivity: `curl -I <url>`.

## H

- **`http <status>: <response text>`** — Generic LLM or HTTP gateway error. Common: `400: unknown provider for model <alias>` → [I ran a command and got an error → `error: llm error`](./tree.md#error-llm-error-http-400-unknown-provider-for-model-alias).

## I

- **`install.source is empty`** — Plugin manifest has no `[source]` section. Manifest schema violation — edit the plugin's `plugin.toml`.
- **`install method is `Unknown` — running binary at <path> was installed in a way Makakoo cannot auto-upgrade.`** — `makakoo upgrade` couldn't classify the binary's install path. The full error lists supported methods. Either reinstall via cargo / homebrew / curl-pipe, or pass `--method <cargo\|brew\|curl-pipe>` to override. Dev builds (`target/debug/`, `target/release/`) are deliberately rejected — use `cargo install --path <checkout>/makakoo` instead.
- **`Invalid API key`** (from the LLM gateway) — [Harvey / MCP not responding → rate limit / resource exhausted](./tree.md#harvey--mcp-not-responding). Specifically: the gateway's stable-key map isn't synced yet; wait 2 seconds or `tytus restart` / `makakoo daemon restart`.

## L

- **`load/create signing key: <error>`** — [I ran a command and got an error → `load/create signing key`](./tree.md#error-loadcreate-signing-key-os-error).

## M

- **`mascot: <name>`** (stray mention in logs) — Diagnostic line from a mascot mission — usually not an error. Check the surrounding context for an actual `error:` or `ok` marker.

## N

- **`no .py entry file in <path>`** — Python plugin entrypoint couldn't be located. `plugin.toml`'s `[entrypoint].start` references a missing file. Reinstall the plugin or edit the manifest.
- **`non-HTTPS install script URL refused: <url>`** — `makakoo upgrade --install-script-url <url>` rejected a non-HTTPS URL. Pass an `https://...` URL; insecure URLs are deliberately blocked.
- **`no $HOME`** — See `cannot resolve $HOME` above.
- **`no current dir available: <error>`** — Your cwd was deleted out from under the process. `cd ~` and retry.
- **`no grant matches path <p>`** — [I ran a command and got an error → `perms revoke` by path](./tree.md#i-ran-a-command-and-got-an-error). Use `makakoo perms list` to confirm the exact scope; revoke by id instead: `makakoo perms revoke <g_id>`.
- **`no grant with id <id>`** — The grant id you passed doesn't exist. `makakoo perms list` for the current id set. (Longer form: `no grant with id <id> — run 'makakoo perms list' to see active grants`.)
- **`non-positive duration <value>; use 30m | 1h | 24h | 7d | permanent`** — Negative or zero duration passed to `perms grant --for`. Pass one of the listed values.

## O

- **`openai-compat template requires --url`** — `makakoo adapter gen openai-compat` needs `--url <http://...>`. Pass it.
- **`outbound::draft body is empty`** — The draft orchestrator received a request with no message body. Caller bug; check the `outbound_draft` invocation.
- **`outbound::draft channel is empty`** — As above, missing `channel` field.
- **`outbound::draft recipient is empty`** — As above, missing `recipient` field.

## P

- **`peer-makakoo template requires --peer-name`** / **`peer-makakoo template requires --url <http://peer-host:port>`** — `makakoo adapter gen peer-makakoo` needs both flags. Pass them.
- **`permanent grant outside $MAKAKOO_HOME (<path>) — pass --yes-really to confirm`** — `permanent` duration is only automatic inside `$MAKAKOO_HOME`. For other paths, confirm with `--yes-really`.
- **`plugin not installed: <name>`** — [I ran a command and got an error → `error: plugin not installed`](./tree.md#error-plugin-not-installed-name).
- **`provide either a grant id or --path`** — You called `makakoo perms revoke` with neither an id nor `--path <p>`. Pass one.

## R

- **`rate limit`** (from `perms grant` or an MCP tool) — [I ran a command and got an error → `error: rate limit`](./tree.md#error-rate-limit).
- **`read trust file: <error>`** — Octopus couldn't read `~/MAKAKOO/config/peers/trusted.keys`. Usually permissions: `ls -la ~/MAKAKOO/config/peers/trusted.keys` should show `-rw------- user`.
- **`reading <path>: <error>`** — Generic file-read failure; `ls -la <path>` and check perms / existence.
- **`refusing to infect $HOME (<path>)`** — `makakoo infect` was asked to write into `$HOME` directly. Create or `cd` into a subdirectory first — `makakoo infect` is scoped to project dirs, not the whole home.
- **`refusing to store empty value`** — You tried to `makakoo secret set <name>` with nothing piped / typed. Pass a non-empty value.
- **`rendered manifest name '<got>' doesn't match requested '<want>'`** — `makakoo adapter gen` produced a manifest whose name field doesn't match what you asked for. File a bug; meanwhile, edit the generated `plugin.toml` to match.
- **`resolve plugins-core: can't find plugins-core/`** — [Plugin install failed → can't find plugins-core](./tree.md#plugin-install-failed).

## S

- **`session <id> has no entries — cannot label`** — Session-tree label was requested on an empty session. Either add at least one entry first or pick an existing session id.
- **`session <id> not found at <path>`** — Session id doesn't exist in the tree store.
- **`setup: unexpected end of input`** — `makakoo setup` got EOF before the section finished (non-TTY stdin, truncated pipe). Rerun interactively or with `--non-interactive`.
- **`sha256 required for tarball sources`** — [Plugin install failed → sha256 required](./tree.md#plugin-install-failed).
- **`shim trust file: out of sync`** — [Octopus peer unreachable → trust store out of sync](./tree.md#octopus-peer-unreachable).
- **`signing key: <error>`** — Octopus identity load failed. See `load/create signing key`.
- **`signature invalid`** / **`401 signature invalid`** — [Octopus peer unreachable → `401 signature invalid`](./tree.md#peer-call-returns-401-signature-invalid).
- **`scope <p> covers the entire home directory — grant a specific subdirectory`** / **`scope <p> is too broad — grant a specific subdirectory`** — [I ran a command and got an error → `error: too broad`](./tree.md#error-too-broad----or-~-home---).
- **`skill '<name>' not found under <dir>`** — `makakoo skill <name>` couldn't locate a matching Python skill. Confirm the name with `makakoo plugin list`; many former "skills" now route through `makakoo plugin info <name>` instead.
- **`skills dir <path> does not exist`** — The registry scan root is missing. Reinstall `lib-harvey-core` or set `MAKAKOO_SKILLS_DIR`.
- **`staging error: target plugin dir already exists — uninstall first`** — [I ran a command and got an error → `staging error`](./tree.md#error-staging-error-target-plugin-dir-already-exists---uninstall-first).
- **`subprocess failed: <label> (exit code <code>)`** — One of the actions queued by `makakoo upgrade` exited non-zero. The label tells you which (`cargo install …`, `brew upgrade …`, `curl … | sh`). Run the action manually to see the full output, fix the root cause, then retry. The chain aborts on first failure — partial upgrades are possible if the kernel succeeds but `makakoo-mcp` fails.
- **`superbrain connection mutex poisoned`** — A thread crashed while holding the DB mutex. `makakoo daemon restart`.

## T

- **`target <path> does not exist — pass --mkdir to create it`** — `makakoo perms grant` needs the path to exist, or you pass `--mkdir`.
- **`template placeholder left unfilled: '<name>'`** — A `makakoo adapter gen` template references a variable that wasn't substituted. Pass the missing flag; see `makakoo adapter gen <template> --help`.
- **`too broad`** — [I ran a command and got an error → `error: too broad`](./tree.md#error-too-broad----or-~-home---).
- **`trust add failed: <error>`** / **`trust remove failed: <error>`** / **`trust file <path>: <error>`** — [Octopus peer unreachable → trust store out of sync](./tree.md#octopus-peer-unreachable).

## U

- **`unknown --format <other> (accepted: markdown, html, json)`** — You passed an unsupported output format. Use one of `markdown`, `html`, `json`.
- **`error: unrecognized subcommand 'upgrade'`** — Your installed binary predates v0.1.3 and doesn't have the `upgrade` verb yet. First-install or `brew upgrade traylinx/tap/makakoo` (or `cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo`) to land v0.1.3+; from then on `makakoo upgrade` works.
- **`unknown provider for model <alias>`** — [I ran a command and got an error → `error: llm error`](./tree.md#error-llm-error-http-400-unknown-provider-for-model-alias).
- **`unknown role '<other>'. Valid: validator, delegate, swarm_member`** — Adapter-manifest `[peer].role` expects one of the three listed values. Edit the manifest.
- **`unknown section in --only: '<name>'. Valid: <list>`** — `makakoo setup --only <name>` was given a section that doesn't exist. Valid sections: `persona`, `brain`, `cli-agent`, `terminal` (macOS), `model-provider`, `infect`.
- **`unknown template '<other>'. Valid: openai-compat, subprocess, mcp-stdio, peer-makakoo`** — `makakoo adapter gen` only knows the four listed templates. Pick one.
- **`unrecognized subcommand '<name>'`** — [I ran a command and got an error → `error: unrecognized subcommand`](./tree.md#error-unrecognized-subcommand-name).
- **`unsupported duration <value>; use 30m | 1h | 24h | 7d | permanent`** — `makakoo perms grant --for` got an unparseable value. Pass one of the listed units.

## W

- **`wait failed: <error>`** — Process-wait syscall failed, usually after an unexpected child exit. Check the child's log at `~/MAKAKOO/data/logs/<child>.err.log`.
- **`WARN skipping plugin — manifest failed to parse`** — Not strictly an error. One of your plugins has a malformed `plugin.toml`. The rest are unaffected. See DOGFOOD-FINDINGS F-006.

## Template-specific

- **`<template> template requires --command <argv>`** — `makakoo adapter gen subprocess` (or `mcp-stdio`) needs `--command "<path> <args…>"`. Pass it.

---

## About this index

This page is the **verbatim-string reference**; the tree is the **fix-by-symptom navigator**. If you're sure what the string is, jump here. If you're fuzzy about wording or have a "it feels wrong" situation, start at [`tree.md`](./tree.md).

Contributors: when you add a new error path to the Rust workspace, update this file in the same PR. The coverage verifier at `scripts/verify_troubleshooting_coverage.py` catches gaps.
