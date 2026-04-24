# `agent-marketing-blog`

**Summary:** Jekyll blog post generator for `jevvellabsblog` — structured launch posts for products, features, sprints.
**Kind:** Agent (plugin) · **Language:** Shell · **Source:** `plugins-core/agent-marketing-blog/`

## When to use

When you want Harvey to draft a **long-form Jekyll blog post** for a product launch, a feature announcement, or a sprint retro. The agent generates a Markdown file with correct frontmatter, sections (hook, problem, solution, proof, CTA), and a suggested image prompt.

Never auto-publishes. Drafts go under `data/marketing/blog/drafts/`; you review and commit to the Jekyll repo yourself.

## Sibling agents

- `agent-marketing-linkedin` — 10-post LinkedIn wheel
- `agent-marketing-twitter` — launch thread (10–14 tweets)

All three share conventions (brief, voice, draft location); only the shape of the output differs.

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-marketing-blog
makakoo plugin disable agent-marketing-blog
makakoo plugin enable agent-marketing-blog
makakoo daemon restart
```

Invoked on-demand via a SANCHO trigger or a direct `~/MAKAKOO/plugins/agent-marketing-blog/src/run.sh <brief-path>` call.

## Where it writes

- **Drafts:** `~/MAKAKOO/data/marketing/blog/drafts/<YYYY-MM-DD-slug>.md`
- **Briefs queue:** `~/MAKAKOO/data/marketing/blog/briefs/`
- **Logs:** `~/MAKAKOO/data/logs/agent-marketing-blog.{out,err}.log`

## Health signals

- Recent files in `drafts/` after a run.
- `run.sh` exit code 0.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Draft has empty sections | LLM timeout on a specific section | Re-run; or reduce the brief's word budget. |
| Frontmatter invalid for Jekyll | Template drift vs Jekyll version | Sync the template in `src/templates/` with your blog repo. |

## Capability surface

- `llm/chat` — prose generation.
- `fs/read` + `fs/write` — brief + draft dirs.

## Remove permanently

```sh
makakoo plugin uninstall agent-marketing-blog --purge
```
