# `agent-career-manager`

**Summary:** Automates job search, active outreach, and career operations — recruiter-pipeline state, outbound drafts, inbound triage.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-career-manager/`

## When to use

When you're actively looking for contract or full-time work and want Harvey to:

- Track inbound recruiter messages (Gmail, LinkedIn).
- Draft tailored replies (never auto-send — always drafts).
- Keep a pipeline state machine: `new → contacted → negotiation → hired | rejected | stale`.
- Schedule follow-ups.

## Prerequisites

- Gmail access configured via `gws` (Google Workspace CLI).
- Optional: LinkedIn scraper credentials (set via `makakoo secret`).
- A CV at a predictable path (default `~/CV/...`).

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-career-manager
makakoo plugin disable agent-career-manager
makakoo plugin enable agent-career-manager
makakoo daemon restart
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-career-manager/pipeline.json` — the authoritative state machine.
- **Drafts:** `~/MAKAKOO/data/agent-career-manager/drafts/` — every pending reply is a file you review and send manually.
- **Logs:** `~/MAKAKOO/data/logs/agent-career-manager.{out,err}.log`

## Health signals

- `ps -ef | grep active_search.py` — one running process.
- `cat ~/MAKAKOO/state/agent-career-manager/pipeline.json | jq '.prospects | length'` — non-zero count after first run.
- Journal lines tagged `[[career-manager]] ...` appear after triage runs.

## Never auto-sends

By design, `agent-career-manager` drafts every reply but **never clicks Send**. You review drafts under `data/agent-career-manager/drafts/`, edit if needed, then send manually. This is a hard constraint — there is no `--auto-send` flag.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| No prospects appear in pipeline after first run | `gws` not authenticated | Run `gws auth login` and retry. |
| Drafts accumulate for one recruiter who already passed | Stale pipeline state | Edit `pipeline.json` to move the prospect to `rejected`, or delete their entry. |
| Noise: agent triages newsletter / notification mail as recruiter messages | Gmail query too permissive | Tighten the query — add `-from:noreply -from:notifications -label:promotions`. |

## Capability surface

- `exec/shell` — invoking `gws` for Gmail.
- `fs/read` + `fs/write` — pipeline + drafts dirs.
- `llm/chat` — draft synthesis.
- `secret/read` — LinkedIn credentials if scraper enabled.

## Remove permanently

```sh
makakoo plugin uninstall agent-career-manager --purge
```

The `pipeline.json` is lost with `--purge`. Back up first if you want to revive it later.
