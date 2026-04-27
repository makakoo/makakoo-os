# `agent-marketing-linkedin`

**Summary:** LinkedIn post generator — 10-post wheel covering launch, insight, feature, personality, use-case, lore, origin, proof, CTA, community.
**Kind:** Agent (plugin) · **Language:** Shell · **Source:** `plugins-core/agent-marketing-linkedin/`

## When to use

When you want Harvey to draft a **10-post LinkedIn wheel** for a product launch. Each post targets one archetype (launch / insight / feature / personality / use-case / lore / origin / proof / CTA / community).

Never auto-posts. Every draft is a file you review and paste manually.

## Sibling agents

See the convention block on [`agent-marketing-blog`](./agent-marketing-blog.md).

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-marketing-linkedin
makakoo plugin disable agent-marketing-linkedin
makakoo plugin enable agent-marketing-linkedin
makakoo daemon restart
```

Invoke on-demand: `~/MAKAKOO/plugins/agent-marketing-linkedin/src/run.sh <brief-path>`.

## Where it writes

- **Drafts:** `~/MAKAKOO/data/marketing/linkedin/drafts/<NN>-<archetype>.md` (NN is 01..10).
- **Briefs:** `~/MAKAKOO/data/marketing/linkedin/briefs/`
- **Logs:** `~/MAKAKOO/data/logs/agent-marketing-linkedin.{out,err}.log`

## Health signals

- All 10 files present in `drafts/` after a complete run.
- Each draft under LinkedIn's soft-limit (3000 chars) — the template asserts length.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Fewer than 10 drafts after run | One archetype's LLM call errored | Re-run only the missing archetypes: `run.sh <brief> --archetypes 03,07`. |
| All posts sound identical | Brief too narrow | Broaden the brief; add concrete anecdotes per archetype. |

## Capability surface

- `llm/chat`.
- `fs/read` + `fs/write` — brief + draft dirs.

## Remove permanently

```sh
makakoo plugin uninstall agent-marketing-linkedin --purge
```
