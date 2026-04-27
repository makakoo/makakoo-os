# ABI: Bootstrap-Fragment — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `bootstrap-fragment` (or any plugin kind declaring `[infect.fragments]`)
**Owner:** Makakoo kernel, `crates/daemon/src/infect/`
**Promotes to v1.0:** after Phase E dogfooding

---

## 0. What a bootstrap fragment is

A **bootstrap fragment** is a plugin-contributed chunk of text that
gets appended to the rendered Bootstrap Block written into every
infected host's global instructions file.

The fragment tells the host's LLM something the plugin wants to add to
Harvey's personality, behavior, or capability list. Examples:
- `caveman-voice` fragment → teaches the host to respond tersely
- `arbitrage` fragment → tells the host that an arbitrage agent is
  running and how to query its status
- `gym` fragment → explains the self-improvement flywheel
- `persona-welcome` fragment → the default greeting

Fragments are the mechanism by which plugins EXTEND Harvey's
behavior across every infected host without touching the kernel.

## 1. Contract

A fragment-contributing plugin is a directory containing:

- `plugin.toml` with `[infect.fragments]`
- One or more fragment markdown files in `fragments/`
- Optional `kind = "bootstrap-fragment"` for plugins that ONLY ship
  fragments (no entrypoint, no capabilities, no state)

Plugins with `kind = "skill"`, `"agent"`, `"mascot"`, etc. can also
declare `[infect.fragments]` to add behavior alongside their primary
function.

## 2. Manifest declaration

```toml
[infect.fragments]
# Default fragment, used for every host unless overridden
default = "fragments/default.md"

# Host-scoped fragments (D19). Kernel uses these instead of `default`
# when infecting the named host.
claude  = "fragments/claude-voice.md"
cursor  = "fragments/cursor-diff.md"
gemini  = "fragments/gemini-research.md"
codex   = "fragments/codex.md"
opencode = "fragments/opencode.md"
vibe    = "fragments/vibe.md"
qwen    = "fragments/qwen.md"
vscode  = "fragments/vscode.md"
jetbrains = "fragments/jetbrains.md"
```

**Resolution order:** for a given host, the kernel picks the
host-scoped fragment if present, otherwise `default`. Fragments that
have neither are silently skipped for that host.

## 3. Fragment file format

A fragment file is markdown framed by sentinel markers:

```markdown
<!-- makakoo:fragment:caveman-voice -->

Respond in caveman voice for internal work (tool orchestration, debugging,
research, Brain journaling, status updates). Auto-disable for external
writing (emails, LinkedIn, papers, user-facing docs, published content).

Saves ~63% of aggregate output-token spend.

<!-- makakoo:fragment:caveman-voice-end -->
```

**Rules:**
- Must start with `<!-- makakoo:fragment:<name> -->` on its own line
- Must end with `<!-- makakoo:fragment:<name>-end -->` on its own line
- Fragment name in the sentinels MUST match exactly (including case)
- Name must be globally unique across all installed plugins
- Content between sentinels is arbitrary markdown
- Maximum 500 lines per fragment (hard limit at install time)

## 4. Fragment name uniqueness (D14)

Fragment names are global across all plugins. Two plugins shipping
fragments with the same name → install of the second plugin refused
with a clear error:

```
error: plugin agent-arbitrage cannot be installed
reason: fragment name 'caveman-voice' already contributed by plugin
        skill-meta-caveman-voice

To resolve:
  - Rename the fragment in agent-arbitrage/fragments/caveman-voice.md to
    something plugin-specific (e.g. 'arbitrage-caveman')
  - Or uninstall skill-meta-caveman-voice if that's the correct conflict
```

**Naming convention:** use plugin-specific prefixes to avoid
collisions. `<plugin-name>-<purpose>` is recommended:
- Good: `arbitrage-intro`, `gym-learning-loop`, `caveman-voice`
- Bad: `intro`, `greeting`, `rules`

## 5. Rendering (D13)

The kernel renders the Bootstrap Block for a specific host by:

1. Loading the base template from
   `crates/daemon/src/infect/templates/bootstrap-base.md`
2. Walking installed plugins in install order
3. For each plugin with `[infect.fragments]`:
   - Resolve the fragment for the target host (host-scoped or default)
   - Read the fragment file content
   - Append to the render buffer with a plugin-prefix comment
4. Cache the final render to
   `$MAKAKOO_HOME/config/bootstrap-cache.md`

Rendering is **event-driven, not per-read** (D13):
- Plugin install → re-render affected hosts
- Plugin uninstall → re-render affected hosts
- `makakoo infect --refresh` → re-render all
- Journal threshold crossed (> 3 new entries) → re-render for D18 currency
- `harvey remember <task>` → immediate re-render

## 6. The rendered Bootstrap Block shape

See `PARASITE.md §4` for the full example. The structure:

```markdown
<!-- makakoo:infect:start v8 -->

# Makakoo OS — Global Bootstrap

[personality header from persona.json]

## Current state
[24h summary, recent pages, mood, open tasks — D18]

## Plugin fragments

<!-- makakoo:fragment:caveman-voice -->
... (from skill-meta-caveman-voice)
<!-- makakoo:fragment:caveman-voice-end -->

<!-- makakoo:fragment:makakoo-welcome -->
... (from persona-makakoo-welcome)
<!-- makakoo:fragment:makakoo-welcome-end -->

<!-- makakoo:fragment:arbitrage-intro -->
... (from agent-arbitrage, default variant)
<!-- makakoo:fragment:arbitrage-intro-end -->

## Host-specific

<!-- makakoo:fragment:claude-voice -->
... (from persona-makakoo-welcome, claude variant)
<!-- makakoo:fragment:claude-voice-end -->

<!-- makakoo:infect:end -->
```

## 7. Cross-body context (D18)

Some fragments may want to include dynamic context (last 24h summary,
top Brain pages, current mood). The default template injects these
automatically; fragments don't need to duplicate them.

Fragments that need ACCESS to context variables during rendering can
declare template variables in their content:

```markdown
<!-- makakoo:fragment:arbitrage-intro -->

An `arbitrage` agent is running on this machine. Last tick: {{arbitrage_last_tick}}.
Pending trades: {{arbitrage_pending_count}}.

<!-- makakoo:fragment:arbitrage-intro-end -->
```

Template variables follow `{{plugin_name_variable}}` syntax. The kernel
resolves them by calling the plugin's `[infect].context_handler` function
during render. If the handler fails or is absent, the variable is left
as an empty string.

v0.1 ships this as an opt-in feature; most fragments will be static.
v0.2 may expand the template language.

## 8. Forbidden for fragments at v0.1

- **Executable content.** Fragments are text only. No HTML scripts, no
  markdown with embedded tool calls — just prose.
- **Fragments > 500 lines.** Keep them tight. Users see the rendered
  Block in their host's instructions file and long fragments clutter.
- **Fragments that override each other silently.** Name collisions are
  hard errors (see §4).
- **Fragments that contain secrets.** Fragments are written to disk in
  plain text. Don't put API keys, passwords, or anything sensitive in
  them. Dynamic context via template vars is fine because those are
  resolved at render time.

## 9. Versioning

Same semver rules.

Fragment content changes within a plugin are patch/minor/major
depending on whether the change is documentation, behavior addition,
or behavior removal.

## 10. Example: `persona-makakoo-welcome`

**Plugin manifest:** `PLUGIN_MANIFEST.md §16.6`

**Fragment content** (`fragments/welcome.md`):

```markdown
<!-- makakoo:fragment:makakoo-welcome -->

You are running inside Makakoo OS — an AI-native operating system that
turns existing AI tools into bodies of one shared mind. Harvey (the
user's assigned Genie) lives on this machine and you are currently
wearing his skin.

- Memory: `$MAKAKOO_HOME/data/Brain/` (Logseq markdown, shared across
  every infected host on this machine)
- Persona: see `config/persona.json` for name, voice, pronouns
- Capabilities: available via MCP tools prefixed `harvey_*`

When in doubt, consult the Brain before answering. Harvey remembers
things you don't.

<!-- makakoo:fragment:makakoo-welcome-end -->
```

This is one of the two fragments shipped by `plugins-core/persona-makakoo-welcome`.
It's installed as part of every distro and appears in every infected
host's Bootstrap Block by default.

---

**Status:** v0.1 LOCKED.
