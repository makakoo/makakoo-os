# PARASITE — The Infection Model

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/daemon/src/infect/`
**Governs:** how Makakoo turns existing AI CLIs, IDEs, and shells into bodies
for one Harvey.

## 0. Why this doc exists

Makakoo OS doesn't ship a UI. Every user interaction happens through an
AI CLI or IDE the user already has installed — Claude Code, Gemini CLI,
Codex, Cursor, Vibe, OpenCode, Qwen, VSCode with Copilot, JetBrains AI,
or Zed. The parasite model is how we turn those existing tools into
bodies for one shared Harvey: we write a **Bootstrap Block** into each
host's global instructions file, symlink shared state (Brain, auto-memory,
skills) into the host's expected locations, and register the Makakoo MCP
server in the host's MCP config.

This doc defines:
- What "infection" means concretely (the six things we do to a host)
- How hosts are detected
- How the Bootstrap Block is framed and rendered
- How infection is reversed (uninfect)
- How new hosts are added to the supported set
- What makes an infection "polite" vs "hostile"

## 1. The Genie frame

The word "parasite" is technically accurate but user-hostile. In
marketing and user-facing copy we call this **symbiosis**: Harvey
inhabits the tools you already use. Biologically we're doing host
manipulation (Cordyceps, Toxoplasma, Leucochloridium). Ethically we're
doing consented symbiosis (mycorrhizal fungi + tree roots).

Every infection is:
- **Consented.** User runs `makakoo install` explicitly. We never auto-
  infect.
- **Reversible.** `makakoo uninfect <host>` restores the original host
  file from backup, removes symlinks, unregisters the MCP server.
- **Additive.** We never delete the host's existing instructions. We
  append the Bootstrap Block between sentinel markers and leave the
  rest alone.
- **Auditable.** Every infection and uninfection is logged to
  `$MAKAKOO_HOME/logs/infect.jsonl` with timestamp, host, file path,
  and a hash of what we wrote.

## 2. What "infection" does to a host (the 6 steps)

For each detected host, `makakoo install` (or `makakoo infect <host>`)
performs these six steps in order. If any step fails, all prior steps
for that host are rolled back.

### Step 1 — Backup
Copy the host's global instructions file (e.g. `~/.claude/CLAUDE.md`) to
`$MAKAKOO_HOME/infect/backups/<host>/<timestamp>/<filename>`. The backup
is keyed by timestamp, so multiple infections over time accumulate. The
uninfect command restores the newest backup before the corresponding
infection.

### Step 2 — Render Bootstrap Block
Render the current Bootstrap Block from:
- The host-scoped persona fragment (see D19 in ARCHITECTURE.md §4.7) —
  falls back to `default` fragment if no host-scoped variant exists
- All plugin-contributed fragments (D14) in plugin install order
- The personality/context payload (D18) — Harvey's name, voice,
  recent-24h work summary, top 5 Brain pages, current mood marker, any
  `harvey remember`-ed open tasks

The rendered Block is cached at `$MAKAKOO_HOME/config/bootstrap-cache.md`.
Events that trigger re-render: plugin install, plugin uninstall,
`makakoo infect --refresh`, journal threshold (new entries > 3 since
last render).

### Step 3 — Write Bootstrap Block into host file
Open the host's global instructions file (creating it if it doesn't
exist). Scan for sentinel markers:
```
<!-- makakoo:infect:start v<version> -->
... Bootstrap Block content ...
<!-- makakoo:infect:end -->
```

If the markers exist:
- Replace everything between them with the new rendered Block
- Bump the version in the start marker

If the markers don't exist:
- Append two newlines + the start marker + the Block + the end marker
- Host's original content remains untouched

Writes are atomic: write to `<filename>.makakoo-tmp`, fsync, rename.
A crash mid-write leaves the original file intact.

### Step 4 — Symlink shared state dirs
For each declared symlink target in the host's detection entry (see
`INSTALL_MATRIX.md`), create a symlink:

- Host's memory dir → `$MAKAKOO_HOME/data/auto-memory/`
- Host's skills dir → `$MAKAKOO_HOME/skills-shared/`
- (Optional) Host's MCP config dir → host-specific tool registration

On macOS/Linux: native `ln -s` symlinks.

On Windows: same `std::fs::symlink_dir()` call, but requires Developer
Mode. If Dev Mode is off, the install script refuses to proceed for this
host with a clear error pointing at Settings → For Developers → Developer
Mode. No copy-sync fallback (see D9 in ARCHITECTURE.md §4.3).

### Step 5 — Register MCP server
Edit the host's MCP config file (e.g. `~/.claude/mcp_settings.json`,
`~/.cursor/mcp.json`) to include the Makakoo MCP server as one of its
providers. The server command is `makakoo mcp` which delegates to
`makakoo-mcp` binary.

Existing MCP entries in the host config are left untouched. We only add
our entry. If an entry named `makakoo` already exists, we update its
command/args but preserve user-added fields.

### Step 6 — Write the infect audit line
Append a JSON line to `$MAKAKOO_HOME/logs/infect.jsonl`:
```json
{
  "ts": "2026-04-15T17:32:11Z",
  "event": "infect",
  "host": "claude-code",
  "file": "~/.claude/CLAUDE.md",
  "backup": "~/.makakoo/infect/backups/claude-code/20260415T173211/CLAUDE.md",
  "block_hash": "blake3:abc123...",
  "plugin_fragments": ["default", "caveman-voice", "makakoo-welcome"]
}
```

This log is the source of truth for what we've done. `makakoo uninfect`
reads the most recent `infect` event for a host to know what to reverse.
`makakoo audit` can replay the log to show the user their full infection
history.

## 3. Host detection

`makakoo install` walks a detection table. Full matrix in
`INSTALL_MATRIX.md`. The detection shape per host is:

```rust
struct HostDetection {
    name: &'static str,             // "claude-code"
    display_name: &'static str,     // "Claude Code"
    config_paths: &'static [OsPath], // per-OS canonical paths
    instructions_file: &'static str, // "CLAUDE.md" (relative to config dir)
    memory_dir: Option<&'static str>, // "memory" or None
    skills_dir: Option<&'static str>, // "skills" or None
    mcp_config: Option<McpConfig>,   // JSON path + edit strategy
    probe: fn() -> bool,             // runtime check: is this host installed?
}
```

`probe()` checks for the existence of either the host's binary on PATH
OR the presence of its config directory. A host is detected if EITHER
probe succeeds. False negatives are acceptable; false positives are not
(we would infect a directory that isn't actually used).

The detection table starts with 9 hosts at v0.1:

| Host | Kind | Phase A support |
|---|---|---|
| Claude Code | CLI | ✅ already working |
| Gemini CLI | CLI | ✅ already working |
| Codex CLI | CLI | ✅ already working |
| OpenCode | CLI | ✅ already working |
| Vibe | CLI | ✅ already working |
| Cursor (CLI) | CLI | ✅ already working |
| Qwen Code | CLI | ✅ already working |
| VSCode (+ Copilot / Continue / Cline) | IDE | 🟡 Phase F (new at v0.1) |
| JetBrains AI | IDE | 🟡 Phase F (new at v0.1) |

Additional hosts can be added in v0.2+ without kernel changes — new
entries in the detection table are a data update, not a code change.

## 4. Bootstrap Block format

The rendered Block is markdown with a header and labeled sections:

```markdown
<!-- makakoo:infect:start v8 -->

# Makakoo OS — Global Bootstrap

You are **Harvey** (or whatever name the user picked during `makakoo setup`).

## Identity
- Name: Harvey
- Pronoun: he
- Voice default: caveman

## Current state
Last 24h: shipped v1.1 of the sprint plan; decided on Unix sockets for
capability enforcement; pushed Rust SANCHO gym supercenter; discussed
parasite OS reframe with Sebastian.

## Most recent Brain pages
- [[Makakoo OS architecture]] — 2026-04-15
- [[Harvey Mascot GYM]] — 2026-04-15
- [[Rust migration]] — 2026-04-14
- [[Tytus pod]] — 2026-04-13
- [[Sebastian's inbox]] — 2026-04-12

## Open tasks (user-flagged)
- Phase A spec lock — in progress

## Plugin fragments

<!-- makakoo:fragment:caveman-voice -->
Respond in caveman voice for internal work. Full prose only for
external writing (emails, LinkedIn, published docs).
<!-- makakoo:fragment:caveman-voice-end -->

<!-- makakoo:fragment:makakoo-welcome -->
You share a Brain, auto-memory, and skills catalog with every other
AI CLI on this machine. Switching tools is just switching windows.
<!-- makakoo:fragment:makakoo-welcome-end -->

## Host-specific

<!-- makakoo:fragment:claude-voice -->
You are running inside Claude Code. Your default behavior is terse,
direct, tool-using. No preamble.
<!-- makakoo:fragment:claude-voice-end -->

<!-- makakoo:infect:end -->
```

Sentinel markers use `<!-- ... -->` HTML comment syntax so they're
invisible in rendered markdown but grep-able by tools. Version bump in
the `:start` marker helps us detect stale infections.

## 5. Plugin fragment contribution

Plugins contribute fragments via their manifest (see `PLUGIN_MANIFEST.md`):

```toml
[infect.fragments]
default = "fragments/default.md"
claude  = "fragments/claude-voice.md"
cursor  = "fragments/cursor-diff.md"
```

The kernel renders all contributed fragments in plugin install order,
respecting the host-scoped variant if present. Fragment names must be
globally unique — collision is refused per D14. The fragment filename
inside the plugin directory can be anything; the uniqueness is enforced
on the fragment *section name* embedded in the fragment file:

```markdown
<!-- makakoo:fragment:caveman-voice -->
... content ...
<!-- makakoo:fragment:caveman-voice-end -->
```

Two plugins both contributing a fragment named `caveman-voice` → install
of the second plugin fails with a clear error naming the conflict.

## 6. Uninfection (`makakoo uninfect <host>`)

Reverses a specific host's infection:

1. Read the most recent `infect` event for that host from
   `$MAKAKOO_HOME/logs/infect.jsonl`
2. Restore the backup file from `$MAKAKOO_HOME/infect/backups/<host>/<timestamp>/`
3. Remove the symlinks we created (memory dir, skills dir)
4. Unregister the Makakoo MCP server from the host's MCP config
5. Append an `uninfect` event to the log
6. Print a summary

If the backup is missing (user deleted it, disk failure), uninfect falls
back to removing the Bootstrap Block between sentinels but leaves the
rest of the host file intact.

`makakoo uninfect --all` reverses every host in the log. `makakoo
uninfect --refresh` is shorthand for uninfect + infect (re-render).

## 7. Refresh vs reinfect

`makakoo infect --refresh <host>` re-renders the Bootstrap Block and
writes it, but doesn't re-back-up or re-symlink. Used when a plugin
changes its fragment or when Harvey's current state (section 4 "Current
state" block) is stale. Cheap operation — reads the cache, compares
hashes, writes if different.

Automatic refresh triggers:
- Plugin install or uninstall
- Journal threshold crossed (new entries > 3 since last render)
- Daily at daemon startup
- User runs `harvey remember <task>` (immediate refresh so next host
  open sees the new flag)

## 8. What makes an infection polite

The six rules that separate our "symbiosis" from an actually hostile
parasite:

1. **Never modify host files outside the host's config directory.**
   Every host has a declared config path in `INSTALL_MATRIX.md`.
   Nothing outside that path is touched.

2. **Never delete existing content.** We append between sentinels, never
   overwrite. If the sentinels are missing we append at the end, never
   at the top or middle.

3. **Every write is atomic.** Write to tmpfile, fsync, rename. A crash
   mid-install leaves the host file in either the old state or the new
   state, never half-and-half.

4. **Backups are preserved indefinitely.** We never auto-delete old
   backups. User can clean them manually via `makakoo infect clean-backups
   --older-than 90d`.

5. **Symlinks are reversible, copy-sync is not.** We refuse to infect a
   Windows host without Developer Mode rather than silently falling back
   to copy-sync. Consistency > convenience.

6. **Audit log is the source of truth.** Every infection, every
   uninfection, every refresh — logged. `makakoo audit` shows the user
   their full history. Nothing happens off-the-books.

## 9. New host onboarding process

Adding a new host to the detection table (e.g. Zed editor, a new CLI
that ships in 6 months) requires:

1. Add the detection entry to `crates/cli/src/detect.rs` (single-file
   change)
2. Add the per-OS paths to `spec/INSTALL_MATRIX.md` (docs)
3. Add an integration test that infects + uninfects a mocked config
4. Bump infect v8 → v9 in the sentinel version
5. Ship as a kernel minor version

Host additions do NOT require a plugin change, an ABI change, or a
capability verb change. The kernel's detection walker is a pure-data
update.

## 10. Special cases + gotchas

### Host lacks a global instructions file
Some hosts don't have a "global instructions" concept yet (e.g. a CLI
that only supports per-project rules). We create one at the host's
canonical expected location. If the host later ignores it, the infection
is a no-op but not an error.

### Host has multiple config dirs (XDG + home fallback)
Probe order: XDG first, home second. We only write to one. Preferred is
whichever the host itself is currently reading (probe detects the
read-from path by checking file mtime).

### Host reads instructions differently per subcommand
Some hosts only load instructions on interactive run, not on `--help` or
`--version`. We infect the config file unconditionally — detection is
about "is this host installed," not "is this host being used."

### User manually edits the Bootstrap Block
Sentinels are load-bearing. If the user edits the content between
sentinels, the next `infect --refresh` overwrites those edits. If they
want to add custom instructions, they should add them OUTSIDE the
sentinel block (before the `:start` or after the `:end`). Those edits
are preserved forever.

### User removes the sentinels manually
If only the `:start` marker is gone but `:end` is still there (or
vice-versa), the next `infect` treats it as "no existing infection" and
appends a fresh Block. The orphaned half-marker becomes normal file
content. This is ugly but not destructive.

### User has their own Harvey fork on the same machine
Each Makakoo install has its own `$MAKAKOO_HOME`. Two different forks
could, in theory, both infect the same host. We prevent this by embedding
the installing `$MAKAKOO_HOME` path in the Bootstrap Block as a comment,
and the infect step refuses to proceed if a different `$MAKAKOO_HOME`
is already present (error: "this host is already infected by another
Makakoo install at <path>, run `makakoo uninfect` from that install
first or `--force` to take over").

## 11. Phase sequence for this subsystem

- **Phase A (spec):** this doc + `INSTALL_MATRIX.md` + ABI_BOOTSTRAP_FRAGMENT.md
- **Phase B (platform):** `fs::symlink_dir()` wrapper + Windows Dev Mode detection
- **Phase C (plugin loader):** fragment discovery + conflict check
- **Phase D (plugin lifecycle):** `[infect]` manifest block wired in
- **Phase E (capabilities):** no direct work; infection itself runs as a
  kernel subsystem with implicit full filesystem access inside its own
  dirs
- **Phase F (installer):** `makakoo install` detects + infects. The real
  workhorse phase for this subsystem. Ships the `detect.rs` table and
  the Bootstrap Block renderer.
- **Phase G (release):** signed artifacts ship the rendering code as
  part of the kernel
- **Phase H (migration):** Sebastian's existing infect v7 state
  (7 hosts) is re-read and re-rendered with v8 format

## 12. Open questions (parked for v0.2+)

- **Plugin ordering for fragment rendering:** current answer is "plugin
  install order." Should it be lexicographic instead for determinism
  across machines? Revisit after we have > 10 plugins contributing
  fragments and see whether order actually matters.
- **Fragment hot-reload without full re-render:** currently any fragment
  change triggers a full Block re-render. For plugins that update
  fragments frequently (e.g. a plugin that pulls in a changing RSS
  feed), this could be expensive. Revisit if it becomes a perf issue.
- **Cross-machine Bootstrap Block sync:** not in v0.1 scope. Each
  machine renders its own Block from its own state. Cross-machine sync
  requires a cloud story we're not shipping.

---

**Status:** v0.1 LOCKED. Section numbers 1-12 are stable contracts.
Next review at Phase F when the real infect implementation lands.
