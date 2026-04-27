# ABI: Skill — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `skill`
**Owner:** Makakoo kernel, `crates/core/src/abi/skill.rs`
**Promotes to v1.0:** after Phase E dogfooding of at least one skill plugin

---

## 0. What a skill is

A **skill** is an on-demand capability invoked by the user or by another
plugin. Skills are stateless at the ABI level (they may keep their own
state dir, but each invocation is independent). Typical skills:
`caveman-voice`, `web-browse`, `inbox-triage`, `market-intel`, `diagrams`.

## 1. Contract

A skill plugin is a directory containing:

- `plugin.toml` with `kind = "skill"`
- `SKILL.md` — human-readable description + usage examples
- An entrypoint executable (script, binary, any language)
- Optional `fragments/` dir for bootstrap contributions
- Optional `state/` dir declared via `[state]` if the skill keeps state

## 2. Minimal manifest

```toml
[plugin]
name = "skill-meta-caveman-voice"
version = "1.0.0"
kind = "skill"
language = "python"
summary = "Terse, token-efficient response mode for internal work"

[source]
path = "plugins-core/skill-meta-caveman-voice"

[abi]
skill = "^0.1"

[depends]
python = ">=3.11"

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
run = ".venv/bin/python -m caveman_voice"

[capabilities]
grants = ["brain/read"]
```

## 3. Invocation contract

The kernel invokes a skill via `makakoo skill run <name> [args]`. The
entrypoint command is taken from `[entrypoint].run`. Arguments passed
after `<name>` are forwarded to the entrypoint as argv.

**Entrypoint environment:**
- `MAKAKOO_HOME` — absolute path to the runtime home
- `MAKAKOO_SOCKET_PATH` — path to the plugin's capability socket
- `MAKAKOO_PLUGIN_NAME` — the plugin's declared name
- `MAKAKOO_PLUGIN_VERSION` — the plugin's declared version
- Standard `PATH`, `HOME`, etc. inherited from the daemon
- **NOT** inherited: any `MAKAKOO_*_KEY` or other secret env (see SECURITY.md §5.3)

**Entrypoint stdin:** empty unless the skill explicitly accepts piped
input (declared via `[entrypoint].accepts_stdin = true`)

**Entrypoint stdout:** captured by the kernel, returned to the caller
of `makakoo skill run`

**Entrypoint stderr:** captured, written to
`$MAKAKOO_HOME/logs/skills/<name>/<timestamp>.stderr` for debugging

**Exit code:** 0 = success. Non-zero = failure; the kernel forwards
the exit code to the caller and logs to audit.

**Timeout:** default 60 seconds. Skills that need longer declare
`[entrypoint].timeout = "10m"`.

## 4. Optional lifecycle hooks

Skills are stateless by default, but can declare optional hooks:

```toml
[entrypoint]
run = ".venv/bin/python -m caveman_voice"
on_install = ".venv/bin/python -m caveman_voice --post-install"
on_uninstall = ".venv/bin/python -m caveman_voice --pre-uninstall"
```

`on_install` runs after the install script succeeds. Used for
first-time setup (e.g. downloading a model, creating config files in
the state dir). Non-zero exit rolls back the install.

`on_uninstall` runs before the plugin dir is removed. Used for cleanup
(e.g. closing network connections, writing a farewell log line).

## 5. Forbidden for skills at v0.1

- **Long-running processes.** Skills that need to stay alive across
  invocations are agents, not skills. Use `kind = "agent"` instead.
- **Registering SANCHO tasks.** Only agents and sancho-task plugins
  register scheduled work. Skills that want scheduled invocation should
  be repackaged as sancho-task plugins.

## 6. Versioning

- Patch (0.1.0 → 0.1.1): documentation clarification
- Minor (0.1.0 → 0.2.0): new optional manifest field (e.g. new
  `[entrypoint]` subkey), new required env var (with backward-compat
  fallback)
- Major (0.1.0 → 1.0.0): renamed field, changed semantics, removed
  capability grant

**v0.1 → v1.0 promotion:** after Phase E when at least one shipped
skill plugin has run through the full lifecycle without schema changes.

## 7. Example: `skill-meta-caveman-voice`

The caveman-voice skill toggles the daemon's default response mode
between caveman (terse) and full (prose).

**Manifest:** see `PLUGIN_MANIFEST.md §16.1`

**Entrypoint behavior:**
```sh
$ makakoo skill run caveman-voice --status
current: caveman
$ makakoo skill run caveman-voice --set full
switched to: full
$ makakoo skill run caveman-voice --set caveman
switched to: caveman
```

**State:** none (mode is stored in `$MAKAKOO_HOME/config/persona.json`'s
`voice_default` field, which the skill reads/writes via
`client.config_read("persona.voice_default")` + `client.config_write(...)`)

**Fragments:** ships a bootstrap fragment
`fragments/caveman-voice.md` that tells infected hosts how to
interpret the caveman mode toggle.

---

**Status:** v0.1 LOCKED.
