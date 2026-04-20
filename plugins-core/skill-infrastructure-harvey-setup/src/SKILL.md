---
name: setup
description: Use when the user types /harvey:setup or asks "set up Harvey", "configure Harvey", "verify my Harvey install", "is Harvey working", or "run the Harvey setup wizard". Walks the user through verifying the MCP plugin install, registering with other CLIs on the box, and (optionally) configuring HarveyChat for Telegram. Calls Harvey's existing CLI tools rather than reimplementing the wizard.
user-invocable: true
allowed-tools:
  - Read
  - Write
  - Bash(ls *)
  - Bash(mkdir *)
  - Bash(python3 *)
  - Bash(bash *)
  - Bash(git *)
---

# /harvey:setup — verify and configure Harvey

This skill is the front door for new and existing Harvey users. It
runs Harvey's own setup tooling — never reinvents it. When you finish,
the user should know exactly what's working, what isn't, and what to
do next.

Arguments passed: `$ARGUMENTS` (currently unused — reserved for
future modes like `--reinstall`, `--telegram-only`, `--check`).

---

## Step 1 — verify HARVEY_HOME and the plugin manifest

```bash
ls "${HARVEY_HOME:-$HOME/HARVEY}/.claude-plugin/plugin.json"
ls "${HARVEY_HOME:-$HOME/HARVEY}/.mcp.json"
```

Both files MUST exist. If either is missing, the `/plugin` screen will
show `harvey MCP · x failed`. Fix:

```bash
cd "${HARVEY_HOME:-$HOME/HARVEY}" && bash install.sh
```

## Step 2 — verify the MCP server is callable

```bash
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}" \
PYTHONPATH="${HARVEY_HOME:-$HOME/HARVEY}/harvey-os" \
python3 -c "
import sys, json
sys.path.insert(0, '${HARVEY_HOME:-$HOME/HARVEY}/harvey-os/core/mcp')
import harvey_mcp
print(f'tools registered: {len(harvey_mcp.TOOLS)}')
print('phase 1+2 harvey_* tools:')
for t in harvey_mcp.TOOLS:
    if t['name'].startswith('harvey_'):
        print(f'  - {t[\"name\"]}')
"
```

Expect at least 10 `harvey_*` tools. If the import errors, dump the
traceback verbatim — it almost always points at a missing Python dep.

## Step 3 — auto-register Harvey with every MCP-capable CLI

```bash
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}" \
PYTHONPATH="${HARVEY_HOME:-$HOME/HARVEY}/harvey-os" \
python3 "${HARVEY_HOME:-$HOME/HARVEY}/harvey-os/core/mcp/setup_mcp.py"
```

This script detects Claude Code, OpenCode, Gemini CLI, and Codex
on the box and registers Harvey via each CLI's native `mcp add`
command. Idempotent — safe to re-run.

Read the printed table to the user. For each CLI:
- ✅ installed → done
- ❌ failed → show the error and suggest the manual command
- skipped → not on this box, no action needed

## Step 4 — quick health check via the swarm runtime

Confirm the swarm wires up cleanly:

```bash
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}" \
PYTHONPATH="${HARVEY_HOME:-$HOME/HARVEY}/harvey-os" \
python3 -c "
import sys, json
sys.path.insert(0, '${HARVEY_HOME:-$HOME/HARVEY}/harvey-os/core/mcp')
import harvey_mcp
out = harvey_mcp.handle_tool('harvey_swarm_status', {})
print(out)
"
```

Expect 6 agents registered (`researcher`, `synthesizer`, `storage`,
`image_gen`, `task_master`, `olibia`). If the count is wrong, run
the test suite:

```bash
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}" \
python3 "${HARVEY_HOME:-$HOME/HARVEY}/tests/test_mcp_server.py"
```

## Step 5 — optional: HarveyChat (Telegram) setup

Ask the user: *"Want to set up Telegram so Harvey can DM you and
appear in groups?"*

If yes, run the existing wizard interactively. **Do not** try to
reimplement bot token entry — the wizard already handles all 5 known
failure modes (supergroup id format, allowlist, HARVEY_HOME tilde
expansion, bot-to-bot filter, requireMention default):

```bash
cd "${HARVEY_HOME:-$HOME/HARVEY}/harvey-os"
python3 -m core.chat setup
python3 -m core.chat doctor
```

`doctor` is a 6-check diagnostic that exits non-zero if anything is
off. Read its output back to the user verbatim.

If they say no, skip this step entirely.

## Step 6 — final summary

Print a tight 5-line status block:

```
Harvey OS — install status
  Plugin manifest:    ✅ ~/MAKAKOO/.claude-plugin/plugin.json
  MCP server:         ✅ <N> tools registered
  CLIs registered:    ✅ <list of CLIs>
  Swarm agents:       ✅ 6/6
  HarveyChat:         <✅ doctor green | ⊘ skipped | ❌ check doctor output>

Next: in Claude Code, run /plugin to confirm `harvey · enabled` and
/mcp to confirm `harvey · connected`. Then try:
  "Use harvey_brain_search to find my notes about <topic>"
```

End with one Olibia line:

```
mcp__harvey__harvey_olibia_speak({
  message: "Harvey setup complete",
  tone: "celebrate"
})
```

---

## Implementation notes

- **Never edit `~/.claude/channels/telegram/access.json` or HarveyChat
  config files by hand.** The wizards (`add-chat`, `sync-claude`) are
  the only supported paths and they encode every fix from the
  2026-04-10 5-fix debugging session.
- **Don't run `python3 -m core.chat start --daemon` from this skill.**
  The user starts the daemon themselves once setup is green.
- **If `HARVEY_HOME` is unset**, default to `~/MAKAKOO`. If neither
  exists, tell the user to clone Harvey first (point them at
  `install.sh` once the public install is live).
- **Skip any step that's already green** unless the user explicitly
  asked to re-run it. This skill is idempotent.
