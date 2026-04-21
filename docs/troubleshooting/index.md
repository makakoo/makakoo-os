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
