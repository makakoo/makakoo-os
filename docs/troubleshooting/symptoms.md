# Symptoms — verbatim error-string index

Every error string the `makakoo` binary (or one of its Rust subsystems) can emit, mapped to the section in [`tree.md`](./tree.md) that has the fix.

Search this page (`Ctrl+F` / `⌘+F`) for the exact wording you saw. If your symptom isn't here, the tree's **categories** are still organized by observable symptom and usually have a hit.

## Agent runtime release diagnostics

- **`DSH V1 exposes the authenticated runtime API; declared channel ingress still requires the Makakoo/Flue channel-adapter slice.`** — The spec contains a channel declaration, but the default DSH runtime does not start that listener. Use `makakoo agent prompt` or a supported trusted adapter.
- **`Flue provider choice <n> is out of range ...; keeping the spec model.`** — The interactive legacy Flue provider selection was invalid. The existing AgentSpec model remains unchanged; rerun creation and choose a listed number if you intended to change it.
- **`Multiple Flue providers detected; auto-selecting '<provider>' (local-first).`** — Legacy Flue compatibility found several providers and selected the local-first candidate. Review the generated `.env` before running the manual Flue process.
- **`No Flue LLM providers detected. Set a provider key or start switchAILocal.`** — The explicitly selected legacy Flue renderer has no usable provider. Start switchAILocal or configure a supported provider before running it.
- **`custom runtime project preserved outside managed roots: <path>`** — Destroy archived the slot but intentionally left a custom external runtime untouched. Review and remove that directory separately only if it is no longer needed.
- **`destroy aborted: could not prove supervisor stopped (<error>); slot files were not moved`** — Shutdown could not be proven. Inspect the supervisor/service-manager error and retry stop; Makakoo refuses to archive live state.
- **`destroy aborted: could not reserve stopped slot (<error>); slot files were not moved`** — Another same-slot supervisor owns or is racing for the runtime lock. Stop it, verify its PID/command, then retry destroy.
- **`destroy aborted: stop returned exit <rc>; slot files were not moved`** — `agent stop` failed. Resolve that stop error first; no slot files were archived.
- **`slot '<slot>': runtime.project_dir must be absolute`** — The slot runtime metadata points at a relative project directory. Recreate the generated slot or replace it with an absolute contained path.
- **`slot is stopped and its unit was removed, but systemctl daemon-reload failed: <error>`** — The process is stopped and the unit file is gone; only systemd's cache refresh failed. Run `systemctl --user daemon-reload` and inspect the reported error.
- **`<slot>: supervisor already owns the runtime lock; duplicate start ignored`** — Another supervisor already owns this slot. Use `makakoo agent status <slot>`; do not start a second copy.

---

## A

- **`--http must be ADDR:PORT or :PORT`** — [Plugin install failed → invalid daemon flag](./tree.md#plugin-install-failed). Or: you passed `--http` without a valid listen spec to the MCP server; pass `:8765` or `0.0.0.0:8765`.
- **`ambiguous path — <N> grants match <path>`** — You called `makakoo perms revoke --path <p>` and multiple grants' scopes cover the same path. Pick an id: `makakoo perms list` → `makakoo perms revoke <g_id>`.
- **`SKILL.md declares entry: '<path>' but file does not exist in <dir>`** — Plugin `SKILL.md` frontmatter names an `entry:` file that isn't present. Either add the file or remove the declaration. Ziggy (mascot) surfaces this class of issue — see [ziggy.md](../mascots/ziggy.md).

## B

- **`blake3 mismatch`** — [Plugin install failed → blake3 mismatch](./tree.md#plugin-install-failed).
- **``bundled adapter `switchailocal` not found``** — The release-bundled adapter catalog could not be located. Re-run `makakoo update --method curl-pipe` or reinstall from `https://makakoo.com/install.sh`; if running from a source checkout, run commands from the repo root or set `MAKAKOO_BUNDLED_ADAPTERS=<repo>/plugins-core/adapters`.

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
- **`failed to initialize venv: {e}`** — SkillSpector bootstrap could not create its Python environment. Confirm `uv` and Python 3.12 are installed, delete `$MAKAKOO_HOME/state/skillspector-venv`, then rerun the command.
- **`failed to run skillspector scan for JSON: {e}`** — Makakoo found SkillSpector but could not execute the JSON scan. Check the executable path, Python venv health, and platform script permissions.
- **`failed to run skillspector scan for SARIF: {e}`** — Makakoo found SkillSpector but could not execute the SARIF scan. Check the executable path, Python venv health, and platform script permissions.
- **`failed to write bootstrap cache: <error>`** — The infect cache path isn't writable. `~/MAKAKOO/cache/infect/` must be writable by your user.
- **`flue output dir {} already exists and is non-empty — refusing to overwrite`** — The `flue` command-line utility was asked to generate or scaffold files in a folder that is not empty. Clean the directory or choose another location.


## G

- **`GET <url>: <err>`** — Network failure during an HTTP GET (plugin install from tarball, harvey_browse download). Check connectivity: `curl -I <url>`.

## H

- **`http <status>: <response text>`** — Generic LLM or HTTP gateway error. Common: `400: unknown provider for model <alias>` → [I ran a command and got an error → `error: llm error`](./tree.md#error-llm-error-http-400-unknown-provider-for-model-alias).

## I

- **`install.source is empty`** — Plugin manifest has no `[source]` section. Manifest schema violation — edit the plugin's `plugin.toml`.
- **`install method is `Unknown` — running binary at <path> was installed in a way Makakoo cannot auto-update.`** — `makakoo update` couldn't classify the binary's install path. The full error lists supported methods. Either reinstall via cargo / homebrew / curl-pipe, or pass `--method <cargo\|brew\|curl-pipe>` to override. Dev builds (`target/debug/`, `target/release/`) are deliberately rejected — use `cargo install --path <checkout>/makakoo` instead.
- **`Invalid API key`** (from the LLM gateway) — [Harvey / MCP not responding → rate limit / resource exhausted](./tree.md#harvey--mcp-not-responding). Specifically: the gateway's stable-key map isn't synced yet; wait 2 seconds or `tytus restart` / `makakoo daemon restart`.

## L

- **`load/create signing key: <error>`** — [I ran a command and got an error → `load/create signing key`](./tree.md#error-loadcreate-signing-key-os-error).

## M

- **`mascot: <name>`** (stray mention in logs) — Diagnostic line from a mascot mission — usually not an error. Check the surrounding context for an actual `error:` or `ok` marker.

## N

- **`no .py entry file in <path>`** — Python plugin entrypoint couldn't be located. `plugin.toml`'s `[entrypoint].start` references a missing file. Reinstall the plugin or edit the manifest.
- **`non-HTTPS install script URL refused: <url>`** — `makakoo update --install-script-url <url>` rejected a non-HTTPS URL. Pass an `https://...` URL; insecure URLs are deliberately blocked.
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

- **`Python 3.12 is missing. Please install it: brew install python@3.12 uv`** — SkillSpector needs Python 3.12 plus `uv`. On macOS run the printed Homebrew command; on Linux/Windows install equivalent Python 3.12 and `uv` packages, then retry.
- **`peer-makakoo template requires --peer-name`** / **`peer-makakoo template requires --url <http://peer-host:port>`** — `makakoo adapter gen peer-makakoo` needs both flags. Pass them.
- **`permanent grant outside $MAKAKOO_HOME (<path>) — pass --yes-really to confirm`** — `permanent` duration is only automatic inside `$MAKAKOO_HOME`. For other paths, confirm with `--yes-really`.
- **`plugin not installed: <name>`** — [I ran a command and got an error → `error: plugin not installed`](./tree.md#error-plugin-not-installed-name).
- **`provide either a grant id or --path`** — You called `makakoo perms revoke` with neither an id nor `--path <p>`. Pass one.

## R

- **`rate limit`** (from `perms grant` or an MCP tool) — [I ran a command and got an error → `error: rate limit`](./tree.md#error-rate-limit).
- **`read trust file: <error>`** — Octopus couldn't read `~/MAKAKOO/config/peers/trusted.keys`. Usually permissions: `ls -la ~/MAKAKOO/config/peers/trusted.keys` should show `-rw------- user`.
- **`reading <path>: <error>`** — Generic file-read failure; `ls -la <path>` and check perms / existence.
- **`refusing to infect $HOME (<path>)`** — `makakoo infect` was asked to write into `$HOME` directly. Create or `cd` into a subdirectory first — `makakoo infect` is scoped to project dirs, not the whole home.
- **`refusing to read keyring entry '{key}' from a non-interactive process because it may open an OS keychain prompt; export {key}=... or set {ALLOW_KEYCHAIN_PROMPT_ENV}=1 to allow the prompt explicitly`** — `makakoo secret get` was called from a background/non-TTY process. Export the same-named env var for automation, rerun interactively, or set `MAKAKOO_SECRET_ALLOW_KEYCHAIN_PROMPT=1` only when you deliberately accept an OS keychain prompt.
- **`refusing to store empty value`** — You tried to `makakoo secret set <name>` with nothing piped / typed. Pass a non-empty value.
- **`rendered manifest name '<got>' doesn't match requested '<want>'`** — `makakoo adapter gen` produced a manifest whose name field doesn't match what you asked for. File a bug; meanwhile, edit the generated `plugin.toml` to match.
- **`resolve plugins-core: can't find plugins-core/`** — [Plugin install failed → can't find plugins-core](./tree.md#plugin-install-failed).

## S

- **`slot '<slot>' gateway process(es) [...] survived shutdown; refusing cleanup`** — Stop could not terminate a same-user process whose command/runtime metadata still identifies it as this slot. Inspect the PID, terminate it manually only after verifying its command, then retry stop/destroy.
- **`slot archived but destroy no longer owns <run-dir>; refusing to unlink its runtime lock`** — The destroy transaction lost its exclusive shutdown guard after archiving. Leave the lock file in place, verify no slot supervisor is running, and restore from the printed archive before retrying.
- **`<slot>: supervisor owns runtime lock after shutdown grace: <error>`** — The supervisor did not release its singleton lock after graceful and identity-checked forced shutdown. Inspect the same-user supervisor PID and service-manager logs; do not destroy until the lock is released.
- **`<slot>: supervisor process(es) [...] survived forced shutdown`** / **`survived shutdown; refusing cleanup`** — A same-user process still matches `agent _supervisor --slot <slot>` after the stop grace. Verify the PID/command, terminate it, and retry; Makakoo intentionally refuses to remove run state while it survives.
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
- **`skillspector executable not found in venv at {:?}`** — SkillSpector bootstrap completed but the expected binary is absent. Delete `$MAKAKOO_HOME/state/skillspector-venv` and rerun so Makakoo recreates it.
- **`skillspector scan JSON command failed`** — SkillSpector ran but did not produce a successful JSON report. Inspect the stderr above it, then rerun with `--no-cache` to rebuild the venv if needed.
- **`skillspector scan SARIF command failed`** — SkillSpector ran but did not produce a successful SARIF report. Inspect the stderr above it, then rerun with `--no-cache` to rebuild the venv if needed.
- **`staging error: target plugin dir already exists — uninstall first`** — [I ran a command and got an error → `staging error`](./tree.md#error-staging-error-target-plugin-dir-already-exists---uninstall-first).
- **`subprocess failed: <label> (exit code <code>)`** — One of the actions queued by `makakoo update` exited non-zero. The label tells you which (`cargo install …`, `brew upgrade …`, `curl … | sh`). Run the action manually to see the full output, fix the root cause, then retry. The chain aborts on first failure — partial updates are possible if the kernel succeeds but `makakoo-mcp` fails.
- **`superbrain connection mutex poisoned`** — A thread crashed while holding the DB mutex. `makakoo daemon restart`.

## T

- **`target <path> does not exist — pass --mkdir to create it`** — `makakoo perms grant` needs the path to exist, or you pass `--mkdir`.
- **`template placeholder left unfilled: '<name>'`** — A `makakoo adapter gen` template references a variable that wasn't substituted. Pass the missing flag; see `makakoo adapter gen <template> --help`.
- **`too broad`** — [I ran a command and got an error → `error: too broad`](./tree.md#error-too-broad----or-~-home---).
- **`trust add failed: <error>`** / **`trust remove failed: <error>`** / **`trust file <path>: <error>`** — [Octopus peer unreachable → trust store out of sync](./tree.md#octopus-peer-unreachable).

## U

- **`unknown --format <other> (accepted: markdown, html, json)`** — You passed an unsupported output format. Use one of `markdown`, `html`, `json`.
- **`error: unrecognized subcommand 'upgrade'`** — Your installed binary predates v0.1.3 and doesn't have the `upgrade` verb yet. First-install or `brew upgrade traylinx/tap/makakoo` (or `cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo`) to land v0.1.3+; from then on `makakoo update` works.
- **`unknown provider for model <alias>`** — [I ran a command and got an error → `error: llm error`](./tree.md#error-llm-error-http-400-unknown-provider-for-model-alias).
- **`unknown role '<other>'. Valid: validator, delegate, swarm_member`** — Adapter-manifest `[peer].role` expects one of the three listed values. Edit the manifest.
- **`unknown section in --only: '<name>'. Valid: <list>`** — `makakoo setup --only <name>` was given a section that doesn't exist. Valid sections: `persona`, `updates`, `brain`, `cli-agent`, `terminal` (macOS), `lope`, `model-provider`, `infect`.
- **`unknown template '<other>'. Valid: openai-compat, subprocess, mcp-stdio, peer-makakoo`** — `makakoo adapter gen` only knows the four listed templates. Pick one.
- **`unrecognized subcommand '<name>'`** — [I ran a command and got an error → `error: unrecognized subcommand`](./tree.md#error-unrecognized-subcommand-name).
- **`unsupported duration <value>; use 30m | 1h | 24h | 7d | permanent`** — `makakoo perms grant --for` got an unparseable value. Pass one of the listed units.
- **`uv is missing. Please install it: brew install python@3.12 uv`** — SkillSpector bootstrap requires `uv`. Install `uv` and Python 3.12, or run plugin lifecycle tests with `--no-skill-scan` when intentionally skipping scans.
- **`uv pip install of SkillSpector failed`** — `uv` could not install the pinned SkillSpector package. Check network/GitHub access and the pinned git ref, then delete `$MAKAKOO_HOME/state/skillspector-venv` and retry.
- **`uv venv creation failed`** — `uv` could not create the SkillSpector venv. Check disk permissions under `$MAKAKOO_HOME/state/`, Python 3.12 availability, and rerun after deleting any partial venv.

## W

- **`wait failed: <error>`** — Process-wait syscall failed, usually after an unexpected child exit. Check the child's log at `~/MAKAKOO/data/logs/<child>.err.log`.
- **`WARN skipping plugin — manifest failed to parse`** — Not strictly an error. One of your plugins has a malformed `plugin.toml`. The rest are unaffected. See DOGFOOD-FINDINGS F-006.

## Template-specific

- **`<template> template requires --command <argv>`** — `makakoo adapter gen subprocess` (or `mcp-stdio`) needs `--command "<path> <args…>"`. Pass it.

---

## Brain sources and OKF

### Source registry

- **`brain source config is not a regular file: <path>`** - `$MAKAKOO_HOME/config/brain_sources.json` is a directory, symlink, or special file. Move it aside, restore a regular JSON file, then retry `makakoo brain list`.
- **`brain source recovery artifact appeared during registry update`** - Another writer or unrelated file appeared while the atomic registry transaction was running. Stop concurrent setup/Brain commands, inspect sibling `.brain_sources.json.*` files, and retry.
- **`brain source recovery marker collision at <path>; move it aside or remove it manually`** - The reserved recovery-marker path already exists and is not owned by the current transaction. Move it aside after inspection; Makakoo will not delete it.
- **`brain source root overlaps existing source <name>: <path>`** - Registered roots cannot be nested inside each other. Run `makakoo brain list`, then choose a non-overlapping folder or remove the old registry entry.
- **`cannot remove canonical source 'default'`** / **`the canonical 'default' source is fixed and cannot be replaced`** - The canonical Brain is permanent. Add or remove a named enrichment source instead.
- **`invalid source name <name>; use 1-64 letters, digits, '.', '_' or '-'`** - Rename the source using only the printed character set.
- **`refusing marker-only brain source recovery because primary is not a regular file: <path>`** - Recovery found a marker but the primary config path is not a file. Move the conflicting path aside and restore `brain_sources.json` before retrying.
- **`refusing non-file brain source recovery artifact <path>; move it aside or remove it manually`** - A reserved recovery path contains a directory, symlink, or special file. Inspect and move it manually.
- **`refusing to discard brain source backup because primary does not match the owned transaction`** - The primary config changed after Makakoo created its backup. Do not delete either copy; compare them and keep the intended registry.
- **`refusing to discard brain source backup because primary is not a regular file: <path>`** - The primary config changed type during recovery. Move the collision aside, restore a regular JSON file, and retry.
- **`refusing to replace non-file brain source config: <path>`** - The registry path is not a normal file. Move it aside instead of forcing replacement.
- **`refusing unowned brain source recovery artifacts in <dir>; move them aside or remove them manually`** - Makakoo found temp/backup files without its ownership marker. Inspect and move them; automatic cleanup deliberately fails closed.
- **`refusing unowned brain source recovery marker <path>; move it aside or remove it manually`** - The marker is not from a transaction Makakoo can prove it owns. Inspect and move it manually.
- **`unsupported brain source type <type>; expected logseq, obsidian, plain, or okf`** - Pick one of the four supported source types.

### OKF validation and export

- **`OKF bundle is not a directory: <path>`** - Point `makakoo brain validate` or `brain add ... okf` at the bundle directory, not a Markdown file or missing path.
- **`refusing invalid OKF bundle: <N> error(s); run 'makakoo brain validate <path> --json'`** - Run the printed validation command, fix every item in `errors`, then register again. Warnings alone are conformant.
- **`frontmatter closing delimiter is missing`** - A concept starts YAML frontmatter with `---` but has no closing `---`. Close the block before the Markdown body.
- **`source <name> is already an OKF bundle; use or copy its original directory`** - OKF-to-OKF export adds no value. Use the registered directory directly or copy it with normal filesystem tools.
- **`unsupported export format <format>; expected 'okf'`** - `makakoo brain export` currently accepts only `--format okf`.
- **`export produced no concepts`** - The source has no exportable concepts. Confirm the registered source and files. With `--public`, add `visibility: public` only to documents intended for sharing.
- **`public export refused: <path> contains likely secret material (<reason>)`** - Remove the credential/private-key content from that public document or remove its public visibility marker. Do not bypass the refusal by publishing manually.
- **`duplicate OKF destination for <path>`** - Two inputs still map to the same portable destination after collision handling. Rename one source document and retry.
- **`output directory is not empty: <path> (pass --force to replace it)`** - Choose an empty destination, or use `--force` only after reviewing what will be replaced.
- **`output directory became non-empty during export`** - Another process wrote into the destination during staging. Stop the writer and retry with a clean directory.
- **`output cannot be a symlink: <path>`** - Export to a real directory path. Symlink destinations are rejected to prevent path escapes.
- **`output cannot overlap source root: <path>`** / **`output cannot overlap auto-memory source: <path>`** - Put the export outside the source and `$MAKAKOO_HOME/data/auto-memory`; otherwise export could recursively ingest or replace its own inputs.
- **`OKF recovery marker collision at <path>; move it aside or remove it manually`** / **`OKF promotion marker collision at <path>; move it aside or remove it manually`** - A reserved sibling marker already exists. Inspect and move it; Makakoo will not guess ownership.
- **`OKF backup recovery artifact appeared during export: <path>`** - A backup path appeared after the export started. Stop concurrent exporters, inspect the sibling recovery artifacts, and retry.
- **`refusing non-directory OKF recovery artifact <path>; move it aside or remove it manually`** - A reserved stage/backup path is not a directory. Inspect and move it manually.
- **`refusing non-file entry in owned OKF bundle: <path>`** - Recovery found a symlink or special entry inside a bundle it was about to remove. Move the bundle aside and inspect it instead of forcing cleanup.
- **`refusing unowned OKF recovery artifact <path>; move it aside or remove it manually`** / **`refusing unowned OKF recovery marker <path>; move it aside or remove it manually`** / **`refusing unowned OKF promotion marker <path>; move it aside or remove it manually`** - The recovery item lacks valid Makakoo ownership metadata. Preserve it for inspection and move it manually.
- **`refusing to discard OKF backup because promoted output is not an owned directory: <path>`** - The promoted output is missing valid ownership metadata or is no longer a directory. Compare output and backup before deciding which to keep.
- **`refusing to discard OKF backup because output does not match the owned promotion`** - Output changed after promotion. Preserve both output and backup, compare them, then resolve manually.

---

## Additional exact diagnostics

These are less common subsystem errors, but they are still searchable here so the verifier keeps public error strings covered.

- **`--specs is mutually exclusive with --telegram-token / --slack-* (use a spec file for declarative channels)`** - Pick one agent-create source: a YAML/TOML AgentSpec, Telegram shorthand, or Slack shorthand. Do not mix spec input with inline transport flags.
- **`--only-kernel and --only-mcp are mutually exclusive`** - `makakoo update` can target one component at a time. Pass only one flag, or omit both to update both binaries.
- **`GitHub API returned <status>: <url>`** - GitHub rejected a release/API request. Check network, auth/rate limit, and that the release/tag exists.
- **`Slack transport requires --slack-app-token`** - Agent creation for Slack needs the app-level token. Re-run with `--slack-app-token <xapp-...>`.
- **`Slack transport requires --slack-bot-token`** - Agent creation for Slack needs the bot token. Re-run with `--slack-bot-token <xoxb-...>`.
- **`Slack transport requires --slack-team`** - Agent creation for Slack needs the workspace/team id. Re-run with `--slack-team <team-id>`.
- **`agent create needs at least one transport: pass --telegram-token <T> OR --slack-bot-token + --slack-app-token + --slack-team OR --specs <path-to-spec>`** - You invoked quick-start creation without a transport. Prefer an AgentSpec with `--specs`; otherwise add the complete Telegram or Slack shorthand flags. DSH V1 preserves those channel declarations but does not start channel listeners.
- **`agent create requires a <SLOT> argument when --specs is not used`** - Inline Telegram/Slack shorthand requires the slot positional argument. With AgentSpec input, omit it and let the spec `name` become the slot id.
- **`DeepSeek Harness dependencies missing at {}; run cd {} && npm install`** - Install the pinned Node dependencies in the generated project, then rerun `makakoo agent validate <slot>`.
- **`DeepSeek Harness runner missing at {}; recreate the slot or restore the generated project`** - The generated runtime is incomplete. Restore its archive or recreate from the canonical AgentSpec.
- **`DeepSeek Harness requires a non-empty switchAILocal model`** - Set the spec model to `switchailocal/<model>`.
- **`DeepSeek Harness routes through switchAILocal; use 'switchailocal/<model>' or an unprefixed switchAILocal model id, got '{}'`** - Cloud-provider prefixes are rejected for the supervised DSH engine. Route the selected model through switchAILocal.
- **Node.js 22.9+ is required for DeepSeek Harness; `node --version` failed** - Install Node.js 22.9 or newer and ensure `node` is on `PATH`, then rerun `makakoo agent validate <slot>`.
- **`agent runtime output must be an absolute path: {}`** - Pass an absolute `--out` path or omit it to use `$MAKAKOO_HOME/agents-dsh/<slot>`.
- **`agent runtime output {} already exists — refusing to overwrite`** / **`deepseek-harness output dir {} already exists and is non-empty — refusing to overwrite`** - Inspect the existing slot and project. Destroy/archive only after confirmation; do not overwrite generated state blindly.
- **`duplicate spec name '{}' in batch: first seen at {}, again at {}`** - Two files in a `--specs` directory resolve to the same slot id. Rename one spec before retrying; batch creation writes nothing.
- **`init-spec requires a TTY (interactive)`** - Run `makakoo agent init-spec` in an interactive terminal, or write YAML/TOML manually and use `validate-spec`.
- **`provider choice must be a number`** / **`provider choice {} is out of range 1..={}`** - The legacy Flue provider picker needs one displayed numeric choice. DSH does not use this picker.
- **`inline secret '{}' contains a forbidden control character`** - Remove newline, carriage-return, or NUL bytes. Store the value in the keyring or environment instead of inline flags.
- **`agent slot '{}' load failed: {}`** / **`slot load: {}`** - The slot TOML is missing or malformed. Inspect `makakoo agent show <slot>` and restore or recreate the canonical slot.
- **`runtime metadata unavailable at {}: {} (is the slot started?)`** - Start the slot and check status before calling `health` or `prompt`.
- **`invalid runtime metadata {}: {}`** / **`runtime metadata does not belong to slot '{}'`** - `runtime.json` is malformed or belongs to another slot. Stop the runtime and recreate it; do not hand-edit metadata.
- **`runtime endpoint must be a non-zero loopback port`** - The runtime contract allows only `127.0.0.1:<non-zero-port>`. Recreate the generated runner if metadata names a remote host or port zero.
- **`runtime token file escapes the generated project directory`** - The token path failed containment validation. Treat the project as tampered and recreate it.
- **`read runtime token {}: {}`** - The per-start token is missing or unreadable. Restart the slot so the runtime writes a fresh mode-0600 token.
- **`slot '{}' is not running (stale runtime metadata for pid {})`** - Remove the failure by restarting the slot; the CLI rejects stale process metadata.
- **`slot '{}' is not responding at 127.0.0.1:{}: {}`** - Check supervisor status, slot logs, and switchAILocal health before restarting.
- **`agent runtime returned {}: {}`** / **`agent runtime returned {} with invalid JSON: {} ({})`** / **`agent runtime response missing 'response'`** - The local DSH endpoint failed its response contract. Preserve the status/body, inspect runtime logs, and report a Makakoo runtime bug.
- **`slot '{}' is not a DeepSeek Harness runtime`** / **`slot '{}' uses the legacy gateway and has no runtime API`** - `makakoo agent prompt` works only for DSH slots. Existing gateway slots keep their legacy channel lifecycle.
- **`slot '{}' uses the legacy Flue engine; run its proxy and dev scripts from {}`** - Flue is not supervised. Run `npm run proxy` and `npx flue dev` from the generated project, or recreate with default DSH.
- **`status file slot '{}' does not match requested slot '{}'`** - The run directory contains cross-slot state. Stop both affected services, preserve the files for diagnosis, then remove only the stale status record.
- **`status read before signal: {error}`** - Foreground shutdown could not read the supervisor snapshot. Check ownership and integrity under `$MAKAKOO_HOME/run/agents/<slot>/`; Makakoo will not guess a PID.
- **`refusing to signal supervisor: status belongs to '{}'`** - The runtime snapshot names another slot. Preserve it for diagnosis and stop the named services through their own slot ids; Makakoo refuses cross-slot signalling.
- **`signal foreground supervisor {}: {error}`** - The direct foreground SIGTERM failed. Check that the recorded PID still belongs to the slot and that the current user owns it, then retry.
- **`remove stale status: {error}`** - Makakoo could not clean `status.json`. Check file ownership and permissions under `$MAKAKOO_HOME/run/agents/<slot>/`.
- **`slot archived but remove ephemeral run state {} failed: {error}`** - The durable slot TOML/data/runtime archive succeeded, but cleanup under `$MAKAKOO_HOME/run/agents/<slot>/` failed. Confirm no supervisor owns the slot, inspect the printed run directory, then remove only that stopped slot's ephemeral directory.
- **`spec declares a voice channel, but the @flue/* adapter is not available in V1. Use a webhook channel + a custom defineTool for Twilio, or remove the channel from the spec. Tracked for V2.`** - This is the legacy Flue renderer's explicit limitation; DSH V1 also does not start voice ingress.
- **`spec declares an email channel, but the @flue/* adapter is not available in V1. Use a webhook channel + a custom defineTool for SMTP/IMAP, or remove the channel from the spec. Tracked for V2.`** - This is the legacy Flue renderer's explicit limitation; DSH V1 also does not start email ingress.
- **`couldn't infer MCP format for ~/{config_dir}/{f} — pass --mcp-format explicitly`** - A custom CLI host has an unknown MCP config shape. Choose the real on-disk schema with `--mcp-format` rather than guessing.
- **`unknown --from host '{other}' (known: grok, codex, vibe, gemini, qwen, claude, opencode)`** - Use one of the listed host presets or pass the custom paths explicitly.
- **`unknown --mcp-format '{other}' (json-mcp-servers | json-opencode | toml-codex | toml-vibe | toml-simple)`** - Pick one of the supported MCP serialization formats.
- **`expanded scope <path> is a single top-level directory - refuse to grant; pick a subdirectory`** - The permission grant was too broad. Grant a project/subdirectory, not a top-level filesystem directory.
- **`fault-injection runner is gated - set MAKAKOO_FAULT_INJECTION=1 to enable. This guard prevents prod from triggering destructive test scenarios.`** - You tried to run destructive test scenarios without the explicit test gate. Only set the env var in a safe test environment.
- **`garage config missing at <path> - run makakoo plugin install --core garage-store first`** - The Garage backing config is absent. Install the `garage-store` core plugin, then rerun the command.
- **`garage <command> failed (exit <code>): <stderr>`** - The underlying `garage` CLI failed. Run the printed command directly, fix the Garage config/daemon/permissions, then retry.
- **`keychain write failed for endpoint <name>: <error>. Re-run with --allow-file-creds to write to <path> (mode 0600), or unlock the keychain and retry.`** - Credential storage failed. Unlock the OS keychain, or intentionally allow file-backed credentials.
- **`no credentials stored for endpoint <name>. Re-run makakoo s3 endpoint add <name> ... or restore from backup.`** - The S3/Garage endpoint exists without credentials. Add it again or restore the credential store.
- **`pattern <path> has empty system + empty user body - nothing to send`** - A Fabric/pattern file has no prompt body. Add system/user content or pick a different pattern.
- **`pattern <name> not found in registry (looked up <path>)`** - The requested pattern is missing. Check the pattern name and refresh/reinstall the pattern plugin.
- **`<path> exists but is kind=<kind>, not pattern`** - The registry entry is not a pattern. Pick a pattern entry or correct the plugin metadata.
- **`<path> has kind=pattern but no [pattern] table`** - The plugin metadata declares a pattern without the required `[pattern]` table. Fix the manifest or reinstall the plugin.
- **`unknown audit kind '<kind>' - see makakoo_core::agents::audit::AuditKind for accepted values`** - An agent audit event used an unsupported kind. Update the caller to emit a valid audit kind.
- **`unknown scenario '<name>' - known: <known>`** - The fault-injection runner was given an unknown scenario. Pick one of the printed known scenario names.

---

## About this index

This page is the **verbatim-string reference**; the tree is the **fix-by-symptom navigator**. If you're sure what the string is, jump here. If you're fuzzy about wording or have a "it feels wrong" situation, start at [`tree.md`](./tree.md).

Contributors: when you add a new error path to the Rust workspace, update this file in the same PR. The coverage verifier at `scripts/verify_troubleshooting_coverage.py` catches gaps.
