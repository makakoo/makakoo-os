# Walkthroughs

Twelve step-by-step guides that take you from a clean install to every major feature Makakoo ships. Each walkthrough is **copy-paste runnable** — every command was executed on a live install before it was documented.

**If this is your first time with Makakoo, read them in order.** Each builds on the one before it.

## Order + dependencies

```
┌────────────────────────────────────────────────┐
│  01 — Fresh install on a new Mac                │
│  (install the `makakoo` binary, verify health)  │
└──────┬─────────────────────────────────────────┘
       │
       ├─────────────────┬──────────────────┬──────────────────┐
       ▼                 ▼                  ▼                  ▼
┌──────────────┐  ┌───────────────┐   ┌───────────────┐  ┌───────────────┐
│ 02 — First   │  │ 03 — Plugins  │   │ 06 — Grants   │  │ 11 — Tytus    │
│ Brain entry  │  │ (list/toggle) │   │ (sandbox +)   │  │ (private pod) │
└──────┬───────┘  └───────┬───────┘   └───────────────┘  └───────────────┘
       │                  │
       ▼                  ▼
┌───────────────┐  ┌────────────────┐
│ 04 — SANCHO   │  │ 08 — Agents    │
│ grows Brain   │  │ (== plugins)   │
└──────┬────────┘  └───────┬────────┘
       │                   │
       ▼                   ▼
┌───────────────┐   ┌────────────────┐
│ 05 — Ask      │   │ 07 — Browse a  │
│ Harvey (LLM)  │   │ website        │
└───────┬───────┘   └────────────────┘
        │
        ├───────────────────┐
        ▼                   ▼
┌───────────────┐   ┌────────────────┐
│ 09 — Ingest a │   │ 10 — Mascot    │
│ document      │   │ mission        │
└───────────────┘   └────────────────┘

┌────────────────────────────────────────────────┐
│  12 — Octopus federation (stub — pending       │
│       octopus-generalize sprint merge)         │
└────────────────────────────────────────────────┘
```

- **Hard dependencies** (must complete first): every walkthrough except 01 requires 01.
- **Soft dependencies** (helpful but not required): noted inline in each walkthrough's "Prerequisites" section.

## The list

| # | Walkthrough | What you'll do | Time |
|---|---|---|---|
| [01](./01-fresh-install-mac.md) | Fresh install on a new Mac | Download the binary, run `makakoo install`, verify three health checks. | ~5 min |
| [02](./02-first-brain-entry.md) | Your first Brain entry | Write a line to today's journal, sync the index, find it with `makakoo search`. Zero LLM required. | ~3 min |
| [03](./03-install-plugin.md) | Plugins: see, toggle, install | List installed plugins, disable one, re-enable it, learn the three `makakoo plugin install` shapes. | ~4 min |
| [04](./04-write-brain-journal.md) | Watch the Brain grow by itself | See SANCHO tasks, fire them once manually, read `makakoo memory stats`. | ~4 min |
| [05](./05-ask-harvey.md) | Ask Harvey a question | Two paths: through an infected AI CLI (grandma), or via `makakoo query` (power user). | ~5–10 min |
| [06](./06-grant-write-access.md) | Grant write access to a folder | Ask for a 1-hour grant, confirm, revoke, see the audit log. | ~3 min |
| [07](./07-browse-website.md) | Open and read a website with Harvey | Start Chrome with CDP, have an AI CLI drive it via `harvey_browse`. | ~6 min |
| [08](./08-use-agent.md) | Use an agent | Agents are plugins with `kind = "agent"`. Inspect, find logs, disable one. | ~5 min |
| [09](./09-ingest-document.md) | Teach Harvey about a document | Feed a PDF through `harvey_knowledge_ingest`, retrieve it later by content. | ~5 min |
| [10](./10-mascot-mission.md) | Meet the mascots, fire one mission | `nursery list`, `sancho tick`, read the `[[Mascot]] …` journal breadcrumb. | ~4 min |
| [11](./11-connect-tytus.md) | Connect a Tytus private pod | Route LLM calls through your own WireGuard-tunneled pod. | ~6 min |
| [12](./12-octopus-federation.md) | **STUB** — Octopus federation | Coming when the `octopus-generalize` sprint Phase 1 merges publicly. | — |

## If you just want to try ONE thing

- **See Makakoo do something fast**: [Walkthrough 02 — first Brain entry](./02-first-brain-entry.md). Zero setup beyond `makakoo install`.
- **Actually use an AI CLI with memory**: [Walkthrough 05 — Ask Harvey (Path 1)](./05-ask-harvey.md). Works with Claude Code / Gemini / any infected CLI.
- **Understand the plumbing**: [Walkthrough 04 — watch the Brain grow](./04-write-brain-journal.md). The proactive task engine in one page.

## Related docs

- [`getting-started.md`](../getting-started.md) — the five-minute one-page install companion (OS-collapsibles, minimal reading). Walkthrough 01 covers the same ground with more depth.
- [`quickstart.md`](../quickstart.md) — 15-minute guide to the daily-use patterns.
- [`user-manual/index.md`](../user-manual/index.md) — reference docs for every `makakoo` subcommand.
- [`troubleshooting/index.md`](../troubleshooting/index.md) — problem index.
- [`faq.md`](../faq.md) — frequently asked questions.
