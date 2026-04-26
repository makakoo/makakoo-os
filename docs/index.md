# Makakoo OS Documentation

> **Many bodies. One mind.**

Welcome to Makakoo OS documentation! Give every AI CLI on your machine the same persistent brain.

---

## 📚 Complete Documentation Map

```
📚 Documentation
│
├── 🚀 Getting Started
│   ├── getting-started.md          ✅ Install in 5 minutes
│   ├── quickstart.md             ✅ 15-minute guide
│   └── walkthroughs/index.md      ✅ 12 copy-paste end-to-end guides
│
├── 🧠 Concepts
│   ├── concepts/index.md         ✅ Architecture overview
│   ├── concepts/sancho.md        ✅ Proactive tasks (SANCHO)
│   ├── concepts/distros.md        ✅ Distribution bundles
│   ├── concepts/ide-integration.md ✅ IDE integrations
│   ├── concepts/architecture.md    ✅ Deep-dive diagrams
│   └── concepts/shared-storage.md  ✅ Shared S3 (garagetytus)
│
├── 🧠💾 Brain
│   └── brain/index.md            ✅ Memory system guide
│
├── 💻 User Manual
│   └── user-manual/index.md      ✅ All CLI commands
│
├── 🔌 Plugins
│   ├── plugins/index.md         ✅ Using plugins
│   └── plugins/writing.md      ✅ Creating plugins
│
├── 🤖 Agents
│   └── agents/index.md          ✅ Per-agent user manuals (15 agents)
│
├── 🦊 Mascots
│   └── mascots/index.md         ✅ Pixel, Cinder, Ziggy, Glimmer, Olibia
│
├── 📡 API Reference
│   └── api/mcp-tools.md         ✅ MCP tools reference
│
├── 🔧 Troubleshooting
│   ├── troubleshooting/index.md   ✅ Common issues
│   └── troubleshooting/uninstall.md ✅ Clean removal
│
└── 👨‍💻 Development
    ├── development/contributing.md ✅ How to contribute
    └── development/changelog.md  ✅ Version history
```

## 📖 Complete Documentation (20 Files)

| Category | Doc | Lines | Status |
|----------|-----|-------|--------|
| **Getting Started** | getting-started.md | 120 | ✅ |
| **Quickstart** | quickstart.md | 150 | ✅ |
| **Concepts** | concepts/index.md | 200 | ✅ |
| **Concepts** | concepts/sancho.md | 380 | ✅ |
| **Concepts** | concepts/distros.md | 350 | ✅ |
| **Concepts** | concepts/ide-integration.md | 200 | ✅ |
| **Concepts** | concepts/architecture.md | 600 | ✅ |
| **Concepts** | concepts/shared-storage.md | 200 | ✅ |
| **Brain** | brain/index.md | 280 | ✅ |
| **User Manual** | user-manual/index.md | 140 | ✅ |
| **Plugins** | plugins/index.md | 200 | ✅ |
| **Plugins** | plugins/writing.md | 480 | ✅ |
| **API** | api/mcp-tools.md | 500 | ✅ |
| **Troubleshooting** | troubleshooting/index.md | 296 | ✅ |
| **Troubleshooting** | troubleshooting/uninstall.md | 155 | ✅ |
| **Development** | development/contributing.md | 200 | ✅ |
| **Development** | development/changelog.md | 100 | ✅ |

**Total: 4,451 lines of documentation** ✅

## 🔑 Quick Navigation

### New to Makakoo?
1. [Install](getting-started.md) → 5 minutes
2. [Quickstart](quickstart.md) → 15 minutes
3. [Concepts](concepts/index.md) → Understand the system

### Using Daily?
1. [CLI Reference](user-manual/index.md) → All commands
2. [Brain Guide](brain/index.md) → Memory system
3. [SANCHO Guide](concepts/sancho.md) → Proactive tasks

### Extending?
1. [Plugin Guide](plugins/index.md) → Using plugins
2. [Plugin Writing](plugins/writing.md) → Create plugins
3. [MCP Tools](api/mcp-tools.md) → API reference

### Problems?
1. [Troubleshooting FAQ](troubleshooting/index.md) → Common issues
2. [Uninstall](troubleshooting/uninstall.md) → Clean removal

## 🏗️ System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         YOU                                             │
│                    Claude / Gemini / OpenCode / etc.                   │
└────────────────────────────┬────────────────────────────────────────┘
                             │ INFECTED
                             ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        MAKAKOO OS                                      │
│                                                                      │
│   ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐    │
│   │    Brain     │  │   SANCHO     │  │      Plugins         │    │
│   │  (memory)    │  │  (tasks)     │  │   (capabilities)     │    │
│   └──────────────┘  └──────────────┘  └──────────────────────┘    │
│                                                                      │
│   ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐    │
│   │ Superbrain   │  │     MCP      │  │    Daemon           │    │
│   │  (search)    │  │   (tools)    │  │   (auto-start)      │    │
│   └──────────────┘  └──────────────┘  └──────────────────────┘    │
└─────────────────────────────────────────────────────────────────────┘
```

## ✨ Key Features

| Feature | Description |
|---------|-------------|
| **Infection** | One command infects 7+ AI CLIs |
| **Brain** | Persistent journals + pages |
| **Superbrain** | FTS5 + vector search + LLM |
| **SANCHO** | Proactive tasks while you sleep |
| **Plugins** | 38 built-in, extensible |
| **Capabilities** | Security sandboxing |
| **Any Model** | Claude, Gemini, GPT, local |

## 🚀 Quick Start

```bash
# Install
curl -fsSL https://makakoo.com/install | sh

# Infect your CLIs
makakoo infect --global

# Query your brain
makakoo query "what did I work on yesterday?"
```

## 📦 Distros

Choose your setup:

| Distro | Plugins | Best For |
|--------|---------|----------|
| minimal | 5 | Beginners |
| core | 15 | Most users |
| sebastian | 25 | Power users |
| creator | 20 | Writers/creators |
| trader | 20 | Traders |

## 🔗 Resources

- [GitHub](https://github.com/makakoo/makakoo-os)
- [Issues](https://github.com/makakoo/makakoo-os/issues)
- [Discussions](https://github.com/makakoo/makakoo-os/discussions)
- [Discord](https://discord.gg/makakoo)

## 📄 License

MIT · Open Source · No telemetry · Local-first
