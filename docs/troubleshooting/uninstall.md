# Complete Uninstall

Remove Makakoo OS from your system.

## Quick Uninstall

```bash
# Remove all infections
makakoo uninfect --global

# Stop and remove daemon
makakoo daemon uninstall

# Remove Makakoo directories
rm -rf ~/.makakoo ~/MAKAKOO
```

## Step-by-Step

### 1. Backup Your Data

**IMPORTANT:** If you want to keep your Brain:

```bash
# Copy to safe location
cp -r ~/MAKAKOO ~/MAKAKOO-backup-$(date +%Y%m%d)

# Or just the Brain
cp -r ~/MAKAKOO/Brain ~/Brain-backup-$(date +%Y%m%d)
```

### 2. Uninfect All CLIs

```bash
# Preview what will be removed
makakoo uninfect --global --dry-run

# Apply
makakoo uninfect --global
```

### 3. Stop Daemon

```bash
# Check if running
makakoo daemon status

# Stop
makakoo daemon stop

# Uninstall from startup
makakoo daemon uninstall
```

### 4. Remove Files

```bash
# Remove Makakoo directories
rm -rf ~/.makakoo
rm -rf ~/MAKAKOO

# Remove homebrew (if installed)
brew uninstall makakoo
brew uninstall makakoo-mcp
```

### 5. Clean Shell Configuration

Depending on your shell, remove these from your `~/.bashrc`, `~/.zshrc`, etc.:

```bash
# Remove environment variables (if you added any)
# Look for:
# export MAKAKOO_HOME=...
# export AIL_BASE_URL=...
# export AIL_API_KEY=...

# Remove aliases (if you added any)
# Look for:
# alias makakoo=...
```

### 6. Restart Terminal

```bash
# Apply shell changes
source ~/.bashrc   # or ~/.zshrc
```

## What Gets Removed

| Item | Location | Removed? |
|------|----------|----------|
| Makakoo binary | `~/.cargo/bin/` | ✅ |
| Config | `~/.makakoo/` | ✅ |
| Your data | `~/MAKAKOO/` | ✅ |
| Infection | Claude/Gemini/etc. config | ✅ |
| Daemon | launchd/systemd | ✅ |
| Shell completions | `~/.bash_completion.d/` | ❌ (manual) |

## Clean Start

After uninstall, you can reinstall fresh:

```bash
curl -fsSL https://makakoo.com/install | sh
```

## Troubleshooting

### Files Still Present

```bash
# Force remove
sudo rm -rf ~/.makakoo ~/MAKAKOO

# Check for hidden files
ls -la ~ | grep makakoo
```

### Daemon Won't Stop

```bash
# Force kill
pkill -f makakoo-daemon

# Check running processes
ps aux | grep makakoo
```

### CLIs Still Infected

```bash
# Check infection files
cat ~/.claude/settings.local.json | grep makakoo

# Manually remove
# Edit the file and remove the Makakoo section
```

## Reinstalling

After complete uninstall, reinstall:

```bash
# Fresh install
curl -fsSL https://makakoo.com/install | sh

# Or from source
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install
```
