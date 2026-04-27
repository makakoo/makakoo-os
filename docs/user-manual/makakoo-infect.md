# `makakoo infect` — CLI reference

`makakoo infect` writes the Makakoo bootstrap block into every AI CLI host's
global instructions file — `~/.claude/CLAUDE.md` for Claude Code,
`~/.gemini/GEMINI.md` for Gemini CLI, and equivalents for OpenCode, Codex,
Vibe, Qwen, and Cursor. It also registers the `harvey` MCP server in each
CLI's MCP config. Once infected, any of those CLIs loads the Harvey persona
and has access to all Makakoo tools automatically.

Infect is idempotent: it diffs the current block against the shipped
version and only writes when the content has changed. It never touches your
shell dotfiles.

## Flag reference

| Flag | Meaning |
|---|---|
| *(none)* | Write bootstrap block + MCP registration to all detected CLIs. |
| `--global` | Explicit form of the default mode. |
| `--mcp` | Write only the MCP server registration; skip the bootstrap markdown. |
| `--verify` | Audit without writing. Exit code 1 when any drift is detected (CI-safe). |
| `--json` | Emit the drift report as structured JSON (only with `--verify`). |
| `--deep` | Extend `--verify` to also audit per-project entries, workspace `.mcp.json` files, and stale worktree records. |
| `--repair` | With `--verify --deep`: apply canonical rewrites to every zombie entry found. |
| `--dry-run` | Preview what would be written without touching any files. |
| `--target <list>` | Restrict to a comma-separated subset: `claude,gemini,codex,opencode,vibe,qwen,cursor`. |
| `--local` | Project-scoped infect: write `.harvey/context.md` + per-CLI derivatives in the nearest project root. |
| `--dir <path>` | Target directory for `--local` (defaults to cwd, walks up to find `.git/`). |
| `--detect-installed-only` | With `--local`: only write derivatives for CLIs that have a `~/.<cli>/` dotdir. |
| `--ignore-derivatives` | With `--local`: upsert a `.gitignore` block listing the six derivative paths. |

## Key use patterns

### Initial infect after install

```sh
# preview what would be written
makakoo infect --dry-run

# apply to all detected CLIs
makakoo infect

# restrict to two CLIs only
makakoo infect --target claude,gemini
```

### CI drift check

```sh
# exits 0 when all CLIs are in sync, exits 1 when any drift detected
makakoo infect --verify

# machine-readable form for scripting
makakoo infect --verify --json | jq '.drifted[]'
```

### Project-local infect (adds Harvey context to one repo)

```sh
cd ~/projects/my-app
makakoo infect --local
# writes .harvey/context.md + CLAUDE.md, GEMINI.md, AGENTS.md, etc.
# use --ignore-derivatives to gitignore the derivatives automatically
makakoo infect --local --ignore-derivatives
```

## Related commands

- [`makakoo-uninfect.md`](makakoo-uninfect.md) — symmetric remove operation
- [`makakoo-mcp.md`](makakoo-mcp.md) — the MCP server that infect registers
- [`setup-wizard.md`](setup-wizard.md) — the `infect` section of the setup wizard calls this

## Common gotcha

**`makakoo infect --verify` reports drift on every run even after a successful `infect`.**
Usually caused by running two different versions of the binary: the installed
CLI version and the build in `target/debug/`. The bootstrap block content
differs between versions, so the newer binary always sees the older block as
stale. Fix: `cargo install makakoo` to get a single consistent binary on
`$PATH`, then re-run `makakoo infect` once to stamp the current version.
