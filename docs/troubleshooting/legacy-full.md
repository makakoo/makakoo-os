# Troubleshooting Guide

Common issues and their solutions.

## First-time install didn't work?

Before anything else, try the three-step self-check:

```sh
makakoo --version     # does the binary exist and run?
makakoo status        # is the daemon up and are plugins loaded?
makakoo health        # deep probe across all subsystems
```

If any step fails, skim the table below before reading further. The
left column is the error message you see; the right column is the
one-line fix.

| Symptom | Fix |
|---|---|
| `command not found: makakoo` | `~/.local/bin` isn't on your PATH. Run `echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc` (for bash, use `~/.bashrc`). |
| `curl: (60) SSL certificate problem` | Your system clock is wrong or your corporate proxy intercepts HTTPS. Fix the clock (`sudo sntp -sS time.apple.com` on macOS) or run the installer from a network without the proxy. |
| `zsh: permission denied: ./install.sh` when installing from source | `chmod +x install.sh` first, or run `sh install.sh`. |
| "makakoo: cannot be opened because Apple cannot verify it" (macOS Gatekeeper) | Right-click the downloaded `makakoo` binary → Open → Open. Then re-run `makakoo install`. The warning only appears once per binary. |
| `makakoo install` says `daemon install failed` | Usually a one-time macOS LaunchAgent permission prompt that got dismissed. Re-run `makakoo daemon install` alone and accept the prompt. |
| Wizard says `not running on a live terminal` | You piped stdin or you're in CI. Re-run from a normal terminal, or use `makakoo setup --non-interactive` to just print current state. |
| `makakoo brain init` can't find Python | Makakoo needs `python3` on PATH for the brain picker. Install via Homebrew (`brew install python3`) or your distro's package manager. |
| `makakoo setup cli-agent` says `npm not found` | Install Node.js from [nodejs.org](https://nodejs.org) — npm ships with it. Then re-run `makakoo setup cli-agent`. |
| `makakoo setup terminal` says `brew not found` (macOS) | Install Homebrew from [brew.sh](https://brew.sh), then re-run. |
| Ubuntu/Debian: missing `libssl` | `sudo apt install libssl-dev ca-certificates` and re-install. |
| After install, your AI CLI doesn't show the Makakoo bootstrap block | Run `makakoo infect --verify` to see drift, then `makakoo infect` to fix. |
| "Makakoo seems installed but `makakoo query` returns nothing" | Your Brain is empty on a fresh install. Try `makakoo journal add "Hello, world"` and then `makakoo query "hello"` again. |

If your symptom isn't in the table, keep reading.

## Quick Diagnostics

Run these first:

```bash
# System health check
makakoo health

# Daemon status
makakoo daemon status

# Plugin status
makakoo plugin list
```

## Installation Issues

### Installation Fails on macOS

**Symptoms:** `curl | sh` fails with permission error

**Fix:**
```bash
# Allow the installer
chmod +x <(curl -fsSL https://makakoo.com/install)
# Or run with sudo
curl -fsSL https://makakoo.com/install | sudo sh
```

### Windows: Developer Mode Required

**Symptoms:** Installation fails on Windows

**Fix:**
1. Open Settings → Privacy & Security → For developers
2. Enable Developer Mode
3. Re-run installation

### Rust Not Found

**Symptoms:** `cargo: command not found`

**Fix:**
```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

---

## Infection Issues

### CLI Not Detected

**Symptoms:** `makakoo infect` doesn't find your CLI

**Fix:**
```bash
# Check if CLI is installed
which claude  # or gemini, opencode, etc.

# Manually specify CLI path
makakoo infect --target claude --path ~/.local/bin/claude
```

### Infection Not Taking Effect

**Symptoms:** CLI doesn't show Makakoo branding

**Fix:**
1. Restart your terminal
2. Check infection status:
   ```bash
   makakoo infect --global --dry-run
   ```
3. Re-infect:
   ```bash
   makakoo uninfect --global
   makakoo infect --global
   ```

---

## Daemon Issues

### Daemon Not Running

**Symptoms:** `makakoo: daemon not running`

**Fix:**
```bash
# Start daemon
makakoo daemon start

# Check status
makakoo daemon status
```

### Daemon Crashes on Startup

**Symptoms:** Daemon exits immediately

**Fix:**
```bash
# View crash logs
cat ~/.makakoo/logs/daemon/crash.log

# Run in foreground to see errors
makakoo daemon run --foreground
```

---

## Plugin Issues

### Plugin Won't Load

**Symptoms:** Plugin missing from `makakoo plugin list`

**Fix:**
```bash
# Check plugin logs
cat ~/.makakoo/logs/plugins/<plugin-name>/error.log

# Reinstall plugin
makakoo plugin uninstall <plugin-name>
makakoo plugin install <plugin-name>
```

### Plugin Permission Denied

**Symptoms:** `permission denied` when running plugin

**Fix:**
```bash
# Check capabilities
makakoo plugin info <plugin-name>

# May need to grant capabilities
# (see docs/plugins/capabilities.md)
```

---

## Brain Issues

### Query Returns No Results

**Symptoms:** `makakoo query` returns empty

**Fix:**
1. Add information to your Brain:
   ```bash
   # Write to journal
   echo "- Today I worked on X" >> ~/MAKAKOO/Brain/journals/$(date +%Y_%m_%d).md
   
   # Create a page
   cat > ~/MAKAKOO/Brain/pages/my-project.md << 'EOF'
   # My Project
   
   ## Status
   In progress
   
   ## Notes
   ...
   EOF
   ```

2. Rebuild index:
   ```bash
   makakoo sancho run index_rebuild
   ```

### Brain Not Accessible

**Symptoms:** `Brain not found` errors

**Fix:**
```bash
# Check Brain directory
ls -la ~/MAKAKOO/Brain/

# Rebuild index
makakoo sancho run index_rebuild
```

---

## SANCHO Task Issues

### Task Not Running

**Symptoms:** Expected task doesn't execute

**Fix:**
```bash
# Check task status
makakoo sancho status

# Check task history
makakoo sancho history --task <task-name>

# Trigger manually
makakoo sancho run <task-name>
```

### Too Many Tasks Running

**Symptoms:** System slow, many background processes

**Fix:**
```bash
# List running tasks
makakoo sancho status

# Disable unnecessary tasks
makakoo plugin disable <task-plugin>
```

---

## Permission / Write-Access Issues

### `write_file rejected: '...' is outside the allowed baseline roots and active grants`

**Symptoms:** An agent tries to write to a path outside
`~/MAKAKOO/data/reports`, `~/MAKAKOO/data/drafts`, `~/MAKAKOO/tmp`, or
`/tmp`, and the call is refused.

**Fix:** Grant access for the path. In conversation:

```
Agent: write_file rejected: '...' is outside the allowed baseline...
       Want me to grant myself 1h access to ~/foo/? Say yes.
You:   yes
```

Or from the CLI:

```bash
makakoo perms grant ~/foo/ --for 1h
```

Never fabricate a grant call yourself — if an agent did without
asking, it's a bug; check `makakoo perms audit --since 10m` for the
audit trail.

### `rate limit: 50 grants created in the last hour (max 50); wait a bit`

**Symptoms:** After many grant/revoke operations, new grants are
refused.

**Fix:** v0.3.1 introduced decrement-on-revoke (50 grant+revoke
cycles no longer lock you out). If you're still hitting this,
you've created 50 grants in an hour without revoking. Either:

```bash
# Wait for the hourly window to roll
# Or revoke the ones you don't need (also releases slots since v0.3.1)
makakoo perms list
makakoo perms revoke <grant-id>
```

### `rate limit: 20 active grants (max 20); revoke some or wait`

**Symptoms:** You have 20 active grants; a 21st is refused.

**Fix:**

```bash
# See what's open
makakoo perms list

# Revoke the stale ones
makakoo perms revoke <grant-id>
# Or revoke all expired (also done by SANCHO every 900s automatically)
makakoo perms purge
```

### `origin_turn_id required on conversational channels`

**Symptoms:** An agent or MCP tool call reports "origin_turn_id
required on conversational channels (plugin=claude-code); this grant
call appears to be agent-initiated without a human turn binding".

**What it means (v0.3.1 + v0.3.2):** the grant call came from a
conversational plugin (`claude-code`, `gemini-cli`, `codex`,
`opencode`, `vibe`, `cursor`, `qwen`, `pi`, `harveychat*`) without
the host-supplied turn id. This is the prompt-injection
defence — a legitimate tool call from the host carries the turn
id; a fabricated call from the agent's own output doesn't.

**Fix:** Usually not an issue in normal use — the infected
bootstrap threads `user_turn_id` through the MCP tool call. If you
hit this:

1. Re-run `makakoo infect --global` to refresh the bootstrap.
2. Check the audit entry:
   ```bash
   makakoo perms audit --since 10m --json | \
     jq 'select(.correlation_id == "reason:missing_origin_turn_id")'
   ```
3. If scripting, use `makakoo perms grant` (plugin=`cli`) — CLI is
   outside the conversational channel set by design.

### `revoke refused: grant g_... is owned by "claude-code", not "gemini-cli"`

**Symptoms (v0.3.3 Phase A):** an agent tries to revoke a grant that
was created by a different plugin — e.g. Gemini CLI trying to revoke
a grant Claude Code issued.

**What it means:** the ownership gate is doing its job — a compromised
or confused plugin can't wipe another agent's grants.

**Fix:** either ask the original owner (the plugin named in the error)
to revoke, or use the CLI admin bypass:

```bash
makakoo perms revoke <grant-id>   # CLI always succeeds
```

The audit trail:

```bash
makakoo perms audit --since 10m --json | \
  jq 'select(.correlation_id == "reason:not_owner")'
```

### `perms_purge_tick: skipped (within Ns cooldown since last tick)`

**Symptoms (v0.3.3 Phase B):** the SANCHO tick logs a skip instead
of purging.

**What it means:** not an error — the idempotency gate fired because
a previous tick ran less than 60s ago (daemon restart, clock skew).
This is the designed behavior and prevents double audit entries for
the same revocations.

**Fix:** none needed. If you want to force an immediate purge, the
CLI bypasses the gate:

```bash
makakoo perms purge
```

### Grants work from `makakoo perms` but not from chat

**Symptoms:** `makakoo perms grant` succeeds; the agent says it
can't grant.

**Fix:** The agent lost its `HARVEY_PLUGIN` / `user_turn_id` wiring
or the infection is stale:

```bash
# Refresh the bootstrap
makakoo infect --global --dry-run   # preview
makakoo infect --global

# Restart the CLI
```

The agent's infected bootstrap sets `HARVEY_PLUGIN` per host; if
that's missing, `grant_write_access` falls through to the
conversational-channels gate and may be refused.

---

## Network Issues

### LLM API Errors

**Symptoms:** `API error` or `connection refused`

**Fix:**
```bash
# Check LLM gateway
curl http://localhost:18080/health

# Update API key
makakoo secret set AIL_API_KEY

# Restart daemon
makakoo daemon restart
```

### Offline Mode Not Working

**Symptoms:** Tasks fail when offline

**Fix:**
```bash
# Some tasks require network
# Check task config:
cat ~/.makakoo/plugins/<plugin>/plugin.toml

# Disable network-required tasks when offline
makakoo sancho pause <task-name>
```

---

## Complete Uninstall

If nothing else works:

```bash
# Full uninstall
makakoo uninfect --global
makakoo daemon uninstall
rm -rf ~/.makakoo ~/MAKAKOO

# If you used homebrew
brew uninstall makakoo
```

---

## Getting Help

If this guide doesn't help:

1. Check [GitHub Issues](https://github.com/makakoo/makakoo-os/issues)
2. Search [Discussions](https://github.com/makakoo/makakoo-os/discussions)
3. Join [Discord](https://discord.gg/makakoo)

### Debug Mode

For detailed debugging:

```bash
# Enable verbose mode
export RUST_LOG=debug
makakoo <command> --verbose

# Capture debug output
makakoo <command> --verbose 2>&1 | tee debug.log
```
