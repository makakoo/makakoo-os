# `agent-marketing-twitter`

**Summary:** Twitter / X launch thread generator — 10–14 tweets under 280 chars each, structured hook → demo → why → features → CTA.
**Kind:** Agent (plugin) · **Language:** Shell · **Source:** `plugins-core/agent-marketing-twitter/`

## When to use

When you want Harvey to draft a **launch thread** for X. Output is one `.md` file with numbered tweets, each validated for the 280-char limit.

Never auto-posts.

## Sibling agents

See [`agent-marketing-blog`](./agent-marketing-blog.md).

## Start / stop

```sh
makakoo plugin info agent-marketing-twitter
makakoo plugin disable agent-marketing-twitter
makakoo plugin enable agent-marketing-twitter
makakoo daemon restart
```

Invoke on-demand: `~/MAKAKOO/plugins/agent-marketing-twitter/src/run.sh <brief-path>`.

## Where it writes

- **Drafts:** `~/MAKAKOO/data/marketing/twitter/drafts/<YYYY-MM-DD-thread>.md`
- **Briefs:** `~/MAKAKOO/data/marketing/twitter/briefs/`
- **Logs:** `~/MAKAKOO/data/logs/agent-marketing-twitter.{out,err}.log`

## Health signals

- Draft file has 10–14 numbered tweet blocks, each under 280 chars.
- `run.sh` exits 0.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Some tweets exceed 280 chars | LLM didn't respect the length guard | The template re-runs any overlength tweet once. If it still fails, edit manually or tighten the brief. |
| Thread feels disjointed | Brief didn't include a clear narrative | Add a one-line narrative to the brief: "This thread takes the reader from pain → demo → features → CTA." |

## Capability surface

- `llm/chat`.
- `fs/read` + `fs/write` — brief + draft dirs.

## Remove permanently

```sh
makakoo plugin uninstall agent-marketing-twitter --purge
```
