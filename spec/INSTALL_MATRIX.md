# INSTALL_MATRIX — Host × OS × Config-Path Lookup

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/cli/src/detect.rs`
**Governs:** which AI CLIs and IDEs Makakoo detects and infects, and
where their config lives on each OS.

---

## 0. What this doc is

A single reference table for every supported host × OS combination.
The kernel's detection layer reads this table (as Rust const data) to
walk installed AI tools and decide what to infect. When a new host is
added or a host changes its config paths, this doc + one file in
`crates/cli/src/detect.rs` get updated — no kernel architecture change
required.

**Terminology:**
- **Host** = an AI CLI, IDE, or program Makakoo can infect
- **Config dir** = where the host keeps its global settings
- **Instructions file** = the markdown/text file the host reads as
  global system prompt (where we write the Bootstrap Block)
- **Memory dir** = where the host keeps per-session memory (symlinked
  to `$MAKAKOO_HOME/data/auto-memory/`)
- **Skills dir** = where the host keeps reusable skills/rules
  (symlinked to `$MAKAKOO_HOME/skills-shared/`)
- **MCP config** = JSON file listing MCP servers the host connects to

## 1. Detection strategy

For each host, detection runs two probes:

1. **Binary probe:** is the host's CLI binary on `PATH`? (`which claude`,
   `which gemini`, etc.)
2. **Config probe:** does the host's canonical config directory exist?

**Host is detected** if EITHER probe succeeds. This catches users who
installed the CLI but haven't run it yet (binary exists, config
doesn't) and users who have config from a previous install (binary
gone, config still around).

**False negatives are acceptable** — a user who symlinks their config
to a non-canonical path won't be detected, but they can run `makakoo
infect <host> --config-path <path>` to force detection.

**False positives are not acceptable** — we never infect a directory
that isn't actually used by the host. If both probes fail, host is
skipped with a log line.

## 2. The nine hosts at v0.1

| # | Host | Kind | v0.1 status |
|---|---|---|---|
| 1 | Claude Code | CLI | ✅ already infected (v7 bootstrap) |
| 2 | Gemini CLI | CLI | ✅ already infected |
| 3 | Codex CLI | CLI | ✅ already infected |
| 4 | OpenCode | CLI | ✅ already infected |
| 5 | Vibe (Mistral) | CLI | ✅ already infected |
| 6 | Cursor (CLI + desktop) | IDE | ✅ CLI infected, desktop via MCP config |
| 7 | Qwen Code | CLI | ✅ already infected (v7, added 2026-04-14) |
| 8 | VSCode | IDE | 🟡 Phase F new — via Copilot/Continue/Cline rules |
| 9 | JetBrains AI | IDE | 🟡 Phase F new — via `AI Assistant` rules file |

Additional hosts can be added in v0.2+ as new rows in this table + new
entries in `detect.rs`.

## 3. The full matrix

Paths below use `~` for the user's home dir. On Windows, `~` is the
closest equivalent (`%USERPROFILE%`). Where a path differs per OS,
each row lists all three.

### 3.1 Claude Code

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.claude/` | `~/.claude/CLAUDE.md` | `~/.claude/memory/` | `~/.claude/skills/` | `~/.claude/mcp_settings.json` |
| Linux | `~/.claude/` | `~/.claude/CLAUDE.md` | `~/.claude/memory/` | `~/.claude/skills/` | `~/.claude/mcp_settings.json` |
| Windows | `%USERPROFILE%\.claude\` | `%USERPROFILE%\.claude\CLAUDE.md` | `%USERPROFILE%\.claude\memory\` | `%USERPROFILE%\.claude\skills\` | `%USERPROFILE%\.claude\mcp_settings.json` |

**Binary probe:** `which claude` / `where claude.exe`
**Instructions file creation:** if missing, create with `# Claude Code
Global Instructions\n\n` + Bootstrap Block
**MCP registration entry:**
```json
{
  "mcpServers": {
    "makakoo": {
      "command": "makakoo",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

### 3.2 Gemini CLI

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.gemini/` | `~/.gemini/GEMINI.md` | `~/.gemini/memory/` | `~/.gemini/skills/` | `~/.gemini/settings.json` |
| Linux | `~/.gemini/` | `~/.gemini/GEMINI.md` | `~/.gemini/memory/` | `~/.gemini/skills/` | `~/.gemini/settings.json` |
| Windows | `%USERPROFILE%\.gemini\` | `%USERPROFILE%\.gemini\GEMINI.md` | `%USERPROFILE%\.gemini\memory\` | `%USERPROFILE%\.gemini\skills\` | `%USERPROFILE%\.gemini\settings.json` |

**Binary probe:** `which gemini` / `where gemini.cmd`
**MCP registration:** nested under `mcpServers` key in `settings.json`
**Note:** Gemini CLI also has `projects.json` and `trusted_hooks.json` —
we don't touch those

### 3.3 OpenAI Codex CLI

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.codex/` | `~/.codex/AGENTS.md` | `~/.codex/memory/` | `~/.codex/skills/` | `~/.codex/config.toml` |
| Linux | `~/.codex/` | `~/.codex/AGENTS.md` | `~/.codex/memory/` | `~/.codex/skills/` | `~/.codex/config.toml` |
| Windows | `%USERPROFILE%\.codex\` | `%USERPROFILE%\.codex\AGENTS.md` | `%USERPROFILE%\.codex\memory\` | `%USERPROFILE%\.codex\skills\` | `%USERPROFILE%\.codex\config.toml` |

**Binary probe:** `which codex` / `where codex.exe`
**Instructions file:** `AGENTS.md` is the community-standard filename
**MCP registration:** Codex uses TOML config, MCP servers under
`[mcp.servers]` table

### 3.4 OpenCode

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.config/opencode/` | `~/.config/opencode/AGENTS.md` | `~/.config/opencode/memory/` | `~/.config/opencode/skills/` | `~/.config/opencode/opencode.json` |
| Linux | `$XDG_CONFIG_HOME/opencode/` or `~/.config/opencode/` | `<config>/AGENTS.md` | `<config>/memory/` | `<config>/skills/` | `<config>/opencode.json` |
| Windows | `%APPDATA%\opencode\` | `%APPDATA%\opencode\AGENTS.md` | `%APPDATA%\opencode\memory\` | `%APPDATA%\opencode\skills\` | `%APPDATA%\opencode\opencode.json` |

**Binary probe:** `which opencode` / `where opencode.exe`
**MCP registration:** nested under `mcpServers` in `opencode.json`

### 3.5 Vibe (Mistral)

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.vibe/` | `~/.vibe/CLAUDE.md` | `~/.vibe/memory/` | `~/.vibe/skills/` | `~/.vibe/trusted_folders.toml` |
| Linux | `~/.vibe/` | `~/.vibe/CLAUDE.md` | `~/.vibe/memory/` | `~/.vibe/skills/` | `~/.vibe/trusted_folders.toml` |
| Windows | `%USERPROFILE%\.vibe\` | `%USERPROFILE%\.vibe\CLAUDE.md` | `%USERPROFILE%\.vibe\memory\` | `%USERPROFILE%\.vibe\skills\` | `%USERPROFILE%\.vibe\trusted_folders.toml` |

**Binary probe:** `which vibe` / `where vibe.exe`
**Note:** Vibe uses `CLAUDE.md` as its instructions filename (Vibe is
Claude-protocol-compat)

### 3.6 Cursor (CLI + desktop)

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.cursor/` | `~/.cursor/rules/makakoo.md` | `~/.cursor/memory/` | `~/.cursor/rules/` | `~/.cursor/mcp.json` |
| Linux | `~/.cursor/` | `~/.cursor/rules/makakoo.md` | `~/.cursor/memory/` | `~/.cursor/rules/` | `~/.cursor/mcp.json` |
| Windows | `%USERPROFILE%\.cursor\` | `%USERPROFILE%\.cursor\rules\makakoo.md` | `%USERPROFILE%\.cursor\memory\` | `%USERPROFILE%\.cursor\rules\` | `%USERPROFILE%\.cursor\mcp.json` |

**Binary probe:** `which cursor` / `where cursor.exe` (desktop install
also creates a CLI shim)
**Instructions file:** Cursor uses `rules/*.md` — we create a dedicated
`makakoo.md` rather than touching the user's other rule files
**MCP registration:** `mcp.json` at config root

### 3.7 Qwen Code

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.qwen/` | `~/.qwen/QWEN.md` | `~/.qwen/memory/` | `~/.qwen/skills/` | `~/.qwen/settings.json` |
| Linux | `~/.qwen/` | `~/.qwen/QWEN.md` | `~/.qwen/memory/` | `~/.qwen/skills/` | `~/.qwen/settings.json` |
| Windows | `%USERPROFILE%\.qwen\` | `%USERPROFILE%\.qwen\QWEN.md` | `%USERPROFILE%\.qwen\memory\` | `%USERPROFILE%\.qwen\skills\` | `%USERPROFILE%\.qwen\settings.json` |

**Binary probe:** `which qwen` / `where qwen.exe`
**Instructions file:** `QWEN.md` (Qwen Code uses its own brand name)
**MCP registration:** nested under `mcpServers` in `settings.json`

### 3.8 VSCode (via Copilot / Continue / Cline)

VSCode has no single "global AI instructions" file — each extension
uses its own. We target three of them at v0.1:

#### 3.8a GitHub Copilot (workspace-level rules)

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/Library/Application Support/Code/User/` | `~/Library/Application Support/Code/User/copilot-instructions.md` | — | — | — |
| Linux | `~/.config/Code/User/` | `~/.config/Code/User/copilot-instructions.md` | — | — | — |
| Windows | `%APPDATA%\Code\User\` | `%APPDATA%\Code\User\copilot-instructions.md` | — | — | — |

**Binary probe:** `which code` / `where code.cmd`
**Instructions file:** Copilot reads per-workspace `copilot-
instructions.md` (in repo) + user-level if set in settings. We write
the user-level file.
**Memory/skills:** not supported by Copilot — sync via MCP instead

#### 3.8b Continue.dev

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/.continue/` | `~/.continue/config.json` (systemMessage field) | `~/.continue/memory/` | — | `~/.continue/config.json` (mcpServers field) |
| Linux | `~/.continue/` | `~/.continue/config.json` | `~/.continue/memory/` | — | `~/.continue/config.json` |
| Windows | `%USERPROFILE%\.continue\` | `%USERPROFILE%\.continue\config.json` | `%USERPROFILE%\.continue\memory\` | — | `%USERPROFILE%\.continue\config.json` |

**Instructions file:** Continue uses a `systemMessage` field inside
`config.json`. We insert the Bootstrap Block content there, framed by
sentinel markers as a multi-line string.

#### 3.8c Cline (Claude Dev)

| OS | Config dir | Instructions file | Memory dir | Skills dir | MCP config |
|---|---|---|---|---|---|
| macOS | `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/` | `...claude-dev/CLAUDE.md` | `...claude-dev/memory/` | `...claude-dev/skills/` | `...claude-dev/mcp_settings.json` |
| Linux | `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/` | same pattern | same | same | same |
| Windows | `%APPDATA%\Code\User\globalStorage\saoudrizwan.claude-dev\` | same pattern | same | same | same |

**Binary probe:** VSCode binary + directory probe for Cline extension
**Note:** paths are long because VSCode extension globalStorage is
nested deeply

### 3.9 JetBrains AI

JetBrains IDEs (IntelliJ, PyCharm, WebStorm, GoLand, etc.) share a
config dir pattern. The AI Assistant plugin reads rules from a
`.md` file under the IDE's config.

| OS | Config dir pattern | Instructions file |
|---|---|---|
| macOS | `~/Library/Application Support/JetBrains/<IDE><Version>/` | `<config>/AI_Assistant/rules.md` |
| Linux | `~/.config/JetBrains/<IDE><Version>/` | `<config>/AI_Assistant/rules.md` |
| Windows | `%APPDATA%\JetBrains\<IDE><Version>\` | `<config>\AI_Assistant\rules.md` |

**Binary probe:** JetBrains doesn't install a single binary — detect by
scanning the config dir root for any `<IDE><Version>` directory
(IntelliJIdea2025.1, PyCharm2025.1, etc.)
**Multiple IDEs:** a user with IntelliJ + PyCharm gets both infected.
Each IDE has its own config dir.
**MCP config:** JetBrains AI doesn't yet support MCP; we ship rules
only at v0.1. Revisit in v0.2 if/when MCP support lands.

## 4. Symlink creation policy

For each detected host, we create symlinks (or junctions on Windows
with Dev Mode):

- Host's memory dir → `$MAKAKOO_HOME/data/auto-memory/`
- Host's skills dir → `$MAKAKOO_HOME/skills-shared/`

**If the host's memory dir already exists as a non-symlink:**
1. Back it up to `$MAKAKOO_HOME/infect/backups/<host>/<timestamp>/memory/`
2. Delete the original
3. Create the symlink

**If the host's memory dir already exists as a symlink to somewhere else:**
Error — refuse with "another tool has claimed this dir, run `makakoo
infect <host> --force` to take over."

**If the host's memory dir is already symlinked to our auto-memory:**
No-op, success.

## 5. The Rust detection table shape

In `crates/cli/src/detect.rs`:

```rust
pub struct HostDef {
    pub id: &'static str,
    pub display_name: &'static str,
    pub binary: &'static str,
    pub paths: HostPaths,
}

pub struct HostPaths {
    pub macos: HostLayout,
    pub linux: HostLayout,
    pub windows: HostLayout,
}

pub struct HostLayout {
    pub config_dir: &'static str,       // "~/.claude"
    pub instructions_file: &'static str, // "CLAUDE.md"
    pub memory_dir: Option<&'static str>, // "memory"
    pub skills_dir: Option<&'static str>, // "skills"
    pub mcp_config: Option<McpConfigLoc>,
}

pub enum McpConfigLoc {
    JsonFile { path: &'static str, key: &'static str }, // .claude/mcp_settings.json, "mcpServers"
    TomlFile { path: &'static str, key: &'static str }, // .codex/config.toml, "mcp.servers"
}

pub const HOSTS: &[HostDef] = &[
    HostDef {
        id: "claude-code",
        display_name: "Claude Code",
        binary: "claude",
        paths: HostPaths {
            macos:   HostLayout { /* ... */ },
            linux:   HostLayout { /* ... */ },
            windows: HostLayout { /* ... */ },
        },
    },
    // ... 8 more entries
];
```

## 6. Adding a new host (future)

When a new AI CLI ships and we want to support it:

1. Add a row to this table with paths for all 3 OSes
2. Add a `HostDef` entry to `crates/cli/src/detect.rs`
3. Write an integration test that infects + uninfects a mocked config
4. Bump the infect version in `crates/cli/src/infect/bootstrap.rs`
5. Release a kernel minor version

**No new plugin, no new ABI, no new capability verb.** Host additions
are pure detection-table updates.

## 7. Host removal

If a host is no longer maintained or too small to justify support:

1. Move its `HostDef` entry to `DEPRECATED_HOSTS` in `detect.rs`
2. Kernel still recognizes the host for uninfection but no longer
   offers to infect it
3. After 2 kernel releases, fully remove

## 8. Known gotchas

**Cursor has TWO config locations.** The CLI version (`cursor` command)
uses `~/.cursor/`, but the Cursor desktop app ALSO maintains
`~/Library/Application Support/Cursor/User/` (or Windows/Linux
equivalent) for its own settings. We only infect `~/.cursor/` because
that's where the CLI reads its instructions. Desktop-only users should
use the Cursor CLI wrapper.

**Windows `%APPDATA%` vs `%USERPROFILE%`.** Some hosts use one, some
the other. This matrix hand-codes the correct one per host rather than
assuming.

**VSCode extensions live under `globalStorage/<publisher>.<ext>/`.** Long
paths. The detection code must walk `globalStorage/` and look for known
extension IDs.

**JetBrains has product-version directories.** A user with IntelliJ 2024
upgrading to IntelliJ 2025 gets a new config dir. We scan for the newest
directory per IDE name and infect that one. Re-run `makakoo infect` to
catch upgrades.

**Qwen Code was added 2026-04-14** — was the 7th CLI in Sebastian's v7
infect rollout. Confirmed paths match the table above.

## 9. Testing

Every row in this table has a corresponding integration test in
`tests/detect/` that:
1. Creates a mock config dir with the expected file layout
2. Runs the detection probe
3. Asserts the detection succeeds
4. Runs the infect flow with the mocked paths
5. Verifies the Bootstrap Block lands in the right file
6. Verifies symlinks point at the right targets
7. Runs uninfect, verifies rollback

CI runs these on all 3 OSes (when Windows tier is live).

## 10. Versioning this doc

**v0.1:** 9 hosts (this ship list).
**v1.0:** promoted when all 9 are tested across all 3 OSes in CI.

New host additions are minor version bumps. Host removals are major
version bumps (breaking change for users who had that host infected).

---

**Status:** v0.1 LOCKED. Next review at Phase F when the detection
table is wired into `makakoo install`.
