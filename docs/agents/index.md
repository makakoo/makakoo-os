# Agents

Makakoo ships ~15 **agent plugins** in `plugins-core/agent-*/`. An agent is a plugin whose `plugin.toml` declares `kind = "agent"` and has a `[entrypoint]` section — the Makakoo daemon spawns it on startup and keeps it alive.

Read the cross-cutting model in [Walkthrough 08 — Use an agent](../walkthroughs/08-use-agent.md) first. Then use this page as a reference when you need to know what a specific agent does.

## The catalog

| Agent | Does what | Docs |
|---|---|---|
| `agent-arbitrage-agent` | Polymarket CLOB monitoring for negative-risk arbitrage | [manual](./agent-arbitrage-agent.md) |
| `agent-browser-harness` | Real-Chrome CDP driver — powers `harvey_browse` | [manual](./agent-browser-harness.md) |
| `agent-career-manager` | Inbound/outbound recruiter workflow (drafts only, never auto-sends) | [manual](./agent-career-manager.md) |
| `agent-dreams` | Nightly Brain consolidation pass | [manual](./agent-dreams.md) |
| `agent-harveychat` | External messaging gateway (Telegram today) | [manual](./agent-harveychat.md) |
| `agent-knowledge-extractor` | Ingest pathway: chunk + embed + persist URLs, PDFs, audio, video | [manual](./agent-knowledge-extractor.md) |
| `agent-marketing-blog` | Jekyll blog post drafter | [manual](./agent-marketing-blog.md) |
| `agent-marketing-linkedin` | 10-post LinkedIn wheel drafter | [manual](./agent-marketing-linkedin.md) |
| `agent-marketing-twitter` | Launch-thread drafter | [manual](./agent-marketing-twitter.md) |
| `agent-meta-harness-agent` | Experimental — spawns bespoke short-lived agents on demand | [manual](./agent-meta-harness-agent.md) |
| `agent-multimodal-knowledge` | Omni describe pipeline (image / audio / video) | [manual](./agent-multimodal-knowledge.md) |
| `agent-octopus-peer` | Signed-MCP peer-federation listener | [manual](./agent-octopus-peer.md) |
| `agent-pg-watchdog` | Postgres health + schema-drift watchdog | [manual](./agent-pg-watchdog.md) |
| `agent-pi` | pi-mono wrapped as a first-class Makakoo worker | [manual](./agent-pi.md) |
| `agent-switchailocal` | Unified local LLM gateway on port 18080 | [manual](./agent-switchailocal.md) |

## Shared template

Every manual on this page follows the same seven sections, in this order:

1. **Summary** — one sentence from the plugin manifest.
2. **When to use** — concrete user intent signals.
3. **Prerequisites** — what must be on the machine first.
4. **Start / stop** — daemon-managed lifecycle + manual overrides.
5. **Where it writes** — state / data / logs paths.
6. **Health signals** — one-line checks you can run to confirm the agent is working.
7. **Common failures** — symptom → cause → fix table.
8. **Capability surface** — declared grants from `plugin.toml`.
9. **Remove permanently** — the exact `plugin uninstall --purge` command and what it deletes.

If an agent's behavior diverges from this template (e.g. `agent-dreams` is actually a SANCHO task, not a long-lived agent), the page names the caveat up front.

## Related docs

- [Walkthrough 08 — Use an agent](../walkthroughs/08-use-agent.md) — the cross-cutting agent lifecycle walkthrough.
- [`mascots/index.md`](../mascots/) — the mascot specialization of agents.
- [`plugins/`](../plugins/) — how agent plugins fit into the broader plugin model.
- [`user-manual/makakoo-plugin.md`](../user-manual/makakoo-plugin.md) — the `makakoo plugin` CLI reference.
