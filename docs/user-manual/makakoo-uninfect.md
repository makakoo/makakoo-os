# `makakoo uninfect` — CLI reference

`makakoo uninfect` is the symmetric inverse of `makakoo infect`. It strips
the Makakoo bootstrap block from every AI CLI host's global instructions
file — `~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md`, and their equivalents
for OpenCode, Codex, Vibe, Qwen, and Cursor. Any prose you wrote above or
below the marker-delimited block is preserved exactly. If the file would be
left empty after removal (i.e. `infect` created it and you added nothing),
the file is deleted entirely.

The MCP server registration is not automatically removed — use
`makakoo mcp` or edit your CLI's MCP config directly to remove the
`harvey` entry if needed.

## Flag reference

| Flag | Meaning |
|---|---|
| *(none)* | Remove from all detected CLI hosts. |
| `--target <list>` | Restrict to a comma-separated subset: `claude,gemini,codex,opencode,vibe,qwen,cursor`. |
| `--dry-run` | Preview what would be removed without touching any files. |

## Key use patterns

### Preview before removing

```sh
# see exactly which files would be touched and what would be stripped
makakoo uninfect --dry-run
```

### Remove from all detected CLI hosts

```sh
makakoo uninfect
```

### Remove from specific CLIs only

```sh
# remove from Claude Code and Gemini only; leave the rest intact
makakoo uninfect --target claude,gemini
```

### Uninfect a project (local mode)

For a project that was infected with `makakoo infect --local`, use the
`--remove` flag of the infect command rather than uninfect:

```sh
cd ~/projects/my-app
makakoo infect --local --remove
# strips the marker block from the local derivatives; .harvey/ is left intact
```

## What is preserved

- Any prose you wrote above the `<!-- makakoo:infect:start -->` marker.
- Any prose you wrote below the `<!-- makakoo:infect:end -->` marker.
- Other sections of the file not touched by infect.

Only the marker-delimited block and the blank lines immediately surrounding
it are removed.

## Related commands

- [`makakoo-infect.md`](makakoo-infect.md) — the inverse: write or update the bootstrap block
- [`makakoo-mcp.md`](makakoo-mcp.md) — remove the MCP server entry separately if needed
- [`setup-wizard.md`](setup-wizard.md) — the infect section of the setup wizard

## Common gotcha

**`makakoo uninfect` reports "no block found" for a CLI you know was infected.**
The block markers (`<!-- makakoo:infect:start -->` / `<!-- makakoo:infect:end -->`)
must be present exactly as written. If you hand-edited the file and accidentally
removed or modified a marker line, uninfect cannot locate the block boundaries.
In that case, open the file manually, find the Harvey/Makakoo bootstrap prose,
and delete it by hand. The file is plain text.
