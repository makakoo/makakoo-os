# Troubleshooting Guide

Common issues and their solutions.

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
