# Getting Started with Makakoo OS

Install Makakoo OS and be running in 5 minutes.

## Prerequisites

- macOS, Linux, or Windows
- At least one AI CLI installed (Claude Code, Gemini CLI, OpenCode, Vibe, Qwen, etc.)
- Internet connection (for installation)

## Step 1: Install

### macOS / Linux

```bash
curl -fsSL https://makakoo.com/install | sh
```

### Windows (Developer Mode required)

```powershell
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

### From Source

```bash
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo build --release
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install
```

## Step 2: Verify Installation

```bash
makakoo --version
makakoo health
```

Expected output:
```
Makakoo OS v0.1.0
✓ Daemon running
✓ Brain accessible
✓ 5 plugins loaded
```

## Step 3: Infect Your CLIs

Infect all detected AI CLIs:

```bash
# Preview what will happen
makakoo infect --global --dry-run

# Apply changes
makakoo infect --global
```

Or infect specific CLIs:

```bash
makakoo infect --target claude,gemini
```

## Step 4: Verify Infection

Each CLI now shows Makakoo OS at startup:

```bash
# Claude Code
claude

# Gemini CLI
gemini

# OpenCode
opencode
```

## Step 5: Ask Your Brain

```bash
# Ask a question (uses FTS + vectors + LLM)
makakoo query "what projects am I working on?"

# Full-text search
makakoo search "polymarket"
```

## What's Next?

### Daily Usage

- [Quickstart Guide](quickstart.md) — Learn the key commands
- [Brain Guide](brain/index.md) — Understand the memory system

### Customization

- [Plugin Guide](plugins/index.md) — Add new capabilities
- [SANCHO Guide](concepts/sancho.md) — Set up proactive tasks

### Troubleshooting

- [FAQ](troubleshooting/index.md) — Common issues
- [Uninstall](troubleshooting/uninstall.md) — Clean removal

## Environment Variables

Makakoo OS uses these environment variables:

| Variable | Default | Purpose |
|----------|---------|---------|
| `MAKAKOO_HOME` | `~/.makakoo/` | Root directory |
| `AIL_BASE_URL` | `http://localhost:18080/v1` | LLM gateway |
| `AIL_API_KEY` | (none) | API key for LLM |

## File Structure

After installation:

```
~/.makakoo/                    # Makakoo OS root
├── run/                       # Runtime files
│   └── plugins/               # Plugin sockets
├── state/                     # State directories
├── plugins/                   # Installed plugins
├── logs/                     # Log files
└── config/                    # Configuration

~/MAKAKOO/                    # Your data (symlink or separate)
├── Brain/                     # Memory (journals + pages)
│   ├── journals/
│   └── pages/
├── data/                      # Agent data
└── skills/                   # Your skills
```

## Need Help?

```bash
# Show all commands
makakoo --help

# Show command help
makakoo <command> --help

# Show system status
makakoo status
```
