# `makakoo handle` — bounded reads from Makakoo handles

`makakoo handle` reads durable Makakoo artifacts without flooding the terminal or an AI context window. The first supported handle type is `agent-artifact://...`, produced by [`makakoo agent-session`](makakoo-agent-session.md).

## Commands

| Command | Purpose |
|---|---|
| `makakoo handle read <handle>` | Read a bounded projection of a durable artifact. |

## Read modes

```bash
makakoo handle read agent-artifact://asa_... --summary
makakoo handle read agent-artifact://asa_... --head 40
makakoo handle read agent-artifact://asa_... --tail 80
makakoo handle read agent-artifact://asa_... --section EVIDENCE
makakoo handle read agent-artifact://asa_... --jsonpath '$.summary'
makakoo handle read agent-artifact://asa_... --section EVIDENCE --json
```

Only one projection mode may be used at a time. If no mode is supplied, `--summary` is implied.

| Flag | Meaning |
|---|---|
| `--summary` | Compact non-empty-line summary. |
| `--head N` | First `N` lines. |
| `--tail N` | Last `N` lines. |
| `--section NAME` | Read an uppercase section such as `SUMMARY`, `EVIDENCE`, `RISKS`, `BLOCKERS`. |
| `--jsonpath PATH` | Read a small JSON path such as `$`, `$.summary`, or `$.items[0]`. |
| `--max-bytes N` | Clamp returned content. Default: `8192`. |
| `--json` | Emit the full structured read envelope. |

## JSON envelope

`--json` returns:

```json
{
  "ok": true,
  "handle": "agent-artifact://asa_...",
  "mode": "section",
  "content": "...",
  "truncated": false,
  "bytes_returned": 123,
  "total_bytes": 123,
  "error": null,
  "error_type": null,
  "available_sections": ["SUMMARY", "EVIDENCE"],
  "value": null
}
```

If a section or JSON path is missing, `ok` is `false` and `error_type` explains the miss. Missing artifacts or unsupported handle schemes are hard errors.
