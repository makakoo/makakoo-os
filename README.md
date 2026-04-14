# Makakoo OS

> Autonomous cognitive extension for any AI CLI. Persistent memory, multi-agent swarm, brain search, Olibia 🦉.

Makakoo OS is an open-source, MIT-licensed, local-first platform. Many bodies, one mind. Built by contributors from everywhere.

**Status:** pre-launch · v0.1.0 · early public cut of the Rust core.

## What this is

Makakoo OS turns any agentic host into a single swarm — persistent Logseq-backed memory, multi-agent orchestration, superbrain query (FTS5 + vector + LLM synthesis), image + omni multimodal, outbound channels, and a guardian owl mascot. One install, one set of tools, one cognitive model across every body in the swarm.

The problem it solves: AI agents don't remember, and they definitely don't share memory with each other. Every session starts from zero. Every agent is a goldfish with its own fishbowl. Makakoo gives every agent the same persistent brain so the assistant at the terminal, the agent on the other laptop, and the autonomous worker on the server all share continuity across tools, machines, projects, and months of context.

Many bodies. One mind.

## Layout

This repo contains the Rust workspace that ships the `makakoo` and `makakoo-mcp` binaries.

- `makakoo-core/` — library crate: platform abstraction, config, `LlmClient`, `EmbeddingClient`, rusqlite DB, superbrain (FTS5 + vectors + graph + memory stack + recall + promoter), persistent event bus, SANCHO proactive task engine, nursery + mascots, gimmick compositor, chat + teloxide, wiki, outbound, telemetry, swarm subsystem.
- `makakoo-mcp/` — MCP stdio server binary. 41 tools. NDJSON JSON-RPC framing. Drop-in replacement for any MCP client (Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen).
- `makakoo/` — umbrella CLI binary. `search`, `query`, `sancho`, `buddy`, `nursery`, `dream`, `promotions`, `skill`, `daemon`, `infect`, `secret`, `mcp`.
- `distribution/` — packaging config: cargo-dist metadata, Homebrew formula, shell installer. See [distribution/README.md](distribution/README.md).

Skills (Python glue calling ffmpeg, yt-dlp, playwright, instructor, etc) live in a separate companion tree and are invoked via subprocess — `makakoo skill <name>` shells out to `python3`. That's a permanent design decision, not a bridge. See [CROSS-PLATFORM-TODOS.md](CROSS-PLATFORM-TODOS.md) for cross-platform gaps and follow-up work.

## Build

```bash
cargo build --release --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Release profile is tuned for size: `lto=true`, `strip=true`, `opt-level="z"`, `codegen-units=1`, `panic="abort"`. Binary sizes as of v0.1.0: `makakoo` ~5.0 MB, `makakoo-mcp` ~4.7 MB.

## Install

```bash
# Homebrew — once the tap is live
brew install makakoo/makakoo/makakoo

# One-liner — once the release site is live
curl -fsSL https://makakoo.com/install.sh | sh

# From source
cargo install --path makakoo
cargo install --path makakoo-mcp
```

## Quick start

```bash
# Search your Brain (requires $MAKAKOO_HOME populated)
makakoo search "your query" --limit 10

# Ask a question — FTS retrieval + LLM synthesis
makakoo query "what did I decide about X?"

# Run the MCP server (delegates to makakoo-mcp)
makakoo mcp

# Install as an auto-starting daemon
makakoo daemon install
makakoo daemon status

# Onboard your CLIs with the bootstrap block
makakoo infect --global --dry-run   # preview
makakoo infect --global             # write
```

## Env

- `MAKAKOO_HOME` — root dir for Brain, config, logs, databases. Falls back to `HARVEY_HOME`, then `~/MAKAKOO`.
- `AIL_BASE_URL` — LLM gateway base URL (default `http://localhost:18080/v1`)
- `AIL_API_KEY` — LLM gateway API key

## The collective

Makakoo OS is a community-built open-source project. Not a company. Not a startup. Not a SaaS. Contributions welcome across code, docs, mascots, design, translation, testing, ideas, Q&A, and sponsorship — every category is a first-class citizen.

- MIT licensed. Forever.
- Local-first. Your data lives on your machine. No telemetry.
- No VC. No acquisition path.
- Every mascot has a named maintainer. Every contributor is recognised.

See [https://makakoo.com/collective/](https://makakoo.com/collective/).

## License

MIT — see [LICENSE](LICENSE).

---

*🦉 Makakoo OS · an open project · MIT · no VC · no telemetry · eat sleep go bananas repeat 🍌*
