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

- **`--from-toml file has slot_id '<file-slot>' but CLI requested slot '<slot>' - they must match`** - The agent-create TOML belongs to a different slot than the CLI flag. Use the slot id from the TOML, or edit the TOML and retry.
- **`--from-toml is mutually exclusive with --telegram-token / --slack-bot-token`** - Pick one agent-create source: a TOML file, Telegram flags, or Slack flags. Do not mix them.
- **`--only-kernel and --only-mcp are mutually exclusive`** - `makakoo update` can target one component at a time. Pass only one flag, or omit both to update both binaries.
- **`GitHub API returned <status>: <url>`** - GitHub rejected a release/API request. Check network, auth/rate limit, and that the release/tag exists.
- **`Slack transport requires --slack-app-token`** - Agent creation for Slack needs the app-level token. Re-run with `--slack-app-token <xapp-...>`.
- **`Slack transport requires --slack-bot-token`** - Agent creation for Slack needs the bot token. Re-run with `--slack-bot-token <xoxb-...>`.
- **`Slack transport requires --slack-team`** - Agent creation for Slack needs the workspace/team id. Re-run with `--slack-team <team-id>`.
- **`agent create needs at least one transport: pass --telegram-token <T> OR --slack-bot-token + --slack-app-token + --slack-team OR --from-toml <path>`** - You invoked `makakoo agent create` without a transport. Add Telegram flags, the complete Slack flag set, or `--from-toml`.
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
