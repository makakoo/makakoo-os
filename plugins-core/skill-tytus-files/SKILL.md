---
name: tytus-files
description: Use this skill whenever the user asks to move, share, push, pull, upload, download, list, or delete files and folders between their Mac and a Tytus pod (their private AI pod). Works in English and Spanish. Trigger phrases include "push this file to pod 2", "send the PDF to my pod", "upload this folder to pod-02", "pull the report from pod 4", "download everything in pod-02's inbox", "what files are on pod 2", "list pod-04 inbox", "delete that file from the pod", "manda el PDF al pod 2", "sube esta carpeta al pod 4", "descarga el reporte del pod 2", "qué hay en el pod 04", "borra ese archivo del pod". Resolves to `tytus push | pull | ls | rm` verbs. Refuses transfers larger than 100 MB and points at the Garage-backed shared filesystem (v0.7) for bulk data. Per-file/folder sharing only — for indexed knowledge (RAG, vector search) route to `harvey_knowledge_ingest` instead.
---

# skill-tytus-files — human-language file sharing with Tytus pods

## What this skill is

A portable skill bundle shipped with makakoo-os. Any MCP-capable CLI
(claude-code, gemini-cli, pi, opencode, codex, qwen, vibe, cursor…) that
can discover SKILL.md files picks this up at session start and thereafter
knows how to translate natural-language file-sharing intent into the
right `tytus` CLI invocation.

Sebastian's mental model: "I have this file on my Mac, put it on pod 2"
→ one sentence to the agent → `tytus push ./file --pod 02`. Same flow
from any CLI, same command under the hood.

## When to use this skill

Match any of these patterns (EN + ES, case-insensitive, slot-filled):

### PUSH — Mac → pod

- "push `<file>` to pod `<N>`" / "push this to pod-`<N>`"
- "send `<file>` to pod `<N>`" / "send the PDF / notes / zip to pod `<N>`"
- "upload `<file>` to pod `<N>`"
- "copy `<file>` to pod `<N>`" / "copy this into pod-`<N>`"
- "share `<file>` with pod `<N>`"
- "give `<file>` to pod `<N>`"
- "move `<file>` to pod `<N>`" (colloquial — treat as push, not remote-rename)
- "drop `<file>` on pod `<N>`"
- "put `<file>` onto pod `<N>`"
- ES: "manda `<archivo>` al pod `<N>`"
- ES: "envía / envia el archivo al pod `<N>`"
- ES: "sube `<archivo>` al pod `<N>`"
- ES: "copia `<archivo>` al pod `<N>`"
- ES: "pasa / pásale `<archivo>` al pod `<N>`"
- ES: "dale `<archivo>` al pod `<N>`"

### PULL — pod → Mac

- "pull `<path>` from pod `<N>`"
- "download `<path>` from pod `<N>`"
- "get `<file>` from pod `<N>`" / "fetch `<file>` from pod `<N>`"
- "grab `<file>` off pod `<N>`"
- "bring back `<file>` from pod `<N>`"
- "copy `<file>` from pod `<N>`" (direction inferred by the "from")
- "save `<file>` from pod `<N>` to my Mac"
- ES: "baja / bájame el archivo del pod `<N>`"
- ES: "descarga `<archivo>` del pod `<N>`"
- ES: "tráeme `<archivo>` del pod `<N>`"
- ES: "consigue `<archivo>` del pod `<N>`"

### LS — list what's on a pod

- "what's on pod `<N>`"
- "list pod `<N>`" / "list pod `<N>`'s inbox"
- "show me files on pod `<N>`"
- "ls pod `<N>`"
- "what files does pod `<N>` have"
- ES: "qué hay en el pod `<N>`" / "que hay en pod `<N>`"
- ES: "lista el pod `<N>`"
- ES: "muéstrame los archivos del pod `<N>`"
- ES: "qué archivos tiene el pod `<N>`"

### RM — delete on pod

- "delete `<path>` from pod `<N>`"
- "remove `<path>` from pod `<N>`"
- "rm `<path>` on pod `<N>`"
- "wipe `<path>` on pod `<N>`"
- "clear `<path>` on pod `<N>`"
- ES: "borra `<archivo>` del pod `<N>`"
- ES: "elimina `<archivo>` del pod `<N>`"
- ES: "quita `<archivo>` del pod `<N>`"

## Decision table — how to resolve a match

Slot-fill these from the user's phrasing:

| Slot | What | How to infer |
|---|---|---|
| `verb` | `push` / `pull` / `ls` / `rm` | trigger table above |
| `local` | local path (push) or dest dir (pull) | whatever file path the user said; default `./` for pull |
| `remote` | pod path | user-given, or default `/app/workspace/inbox/` for ls; required for pull + rm |
| `pod` | `01..99` | `pod 2` → `02`, `pod-04` → `04`. If user didn't say a pod and exactly ONE is connected, omit `--pod` and let the CLI auto-pick. If user didn't say a pod and multiple are connected, ask which one — never guess. |

Then emit ONE of these shapes:

```bash
tytus push <LOCAL> --pod <NN> [--to /app/workspace/<DEST>]
tytus pull <REMOTE> --pod <NN> [--to <LOCAL_DEST>]
tytus ls [<REMOTE>] [--pod <NN>]    # default remote: /app/workspace/inbox/
tytus rm <REMOTE> --pod <NN> [--recursive]
```

Always pass `--json` when you need a machine-parseable answer for the
agent's own downstream logic. Human-facing output (prints directly to
the terminal) is fine without `--json`.

## Hard limits (baked into the CLI — respect when advising)

- **100 MB ceiling per transfer.** Refused with a clear error pointing
  at the v0.7 Garage sprint. Don't retry; tell the user to use the
  Garage-backed shared filesystem for bulk data (not live yet).
- **`/app/workspace/` is the only writable root on the pod.** Any
  remote path outside this is refused before sending bytes.
- **`..` segments and NUL bytes are refused.** Path validation is
  strict.
- **Directories require `--recursive` on `rm`.** Refuses otherwise.
- **Pods run `dash`.** The CLI handles shell escaping; don't splice
  unescaped user input if you ever bypass the CLI.

## When NOT to use this skill

Route elsewhere when the user's intent is:

- **"Remember this document"** / "index this so you can search it later"
  → `harvey_knowledge_ingest`. That's the RAG path (embeddings + Qdrant).
  `tytus push` drops raw bytes in `/app/workspace/inbox/` with no indexing.
- **"I need a shared folder everyone can read/write"** → v0.7 Garage
  sprint (not shipped yet). Today's `tytus push`/`pull` is per-file.
- **"Send this to someone else (not a pod)"** → outbound-email,
  Telegram, Slack, or regular file sharing — this skill only handles
  pod transfers.
- **"Put this in the pod's database"** → the agent running on the pod
  has its own workspace and tools; `tytus push` lands the file in
  `/app/workspace/inbox/` where the agent can pick it up, but you'd
  typically follow with an agent-driven instruction like "process the
  file I just sent".

## Example dialogues

**EN, single pod connected:**

> User: *"Push ~/report.pdf to pod 2"*
> You:
> ```bash
> tytus push ~/report.pdf --pod 02
> ```
> Then read the CLI's stderr for the result (`pushed ~/report.pdf → pod-02:/app/workspace/inbox/report.pdf`).

**EN, multiple pods — ask first:**

> User: *"Send the meeting notes to my pod"*
> You: *"You have pods 02 and 04 connected — which one? Or both?"*
> User: *"02 please"*
> You:
> ```bash
> tytus push ~/meeting-notes.md --pod 02
> ```

**EN, directory:**

> User: *"Upload my whole project folder to pod-04"*
> You:
> ```bash
> tytus push ~/Projects/acme --pod 04
> ```
> Note: this tars + gzips locally, ships as one blob, untars on the pod
> into `/app/workspace/inbox/acme/`. Size ceiling still applies.

**ES, pull:**

> User: *"Descarga el reporte del pod 2"*
> You: *"¿Qué ruta? `/app/workspace/inbox/reporte.pdf` o hay varios reportes?"*
> User: *"El de la inbox."*
> You:
> ```bash
> tytus pull /app/workspace/inbox/reporte.pdf --pod 02
> ```

**EN, ls with default path:**

> User: *"What's on pod 4?"*
> You:
> ```bash
> tytus ls --pod 04
> ```

**EN, rm — always confirm for directories:**

> User: *"Delete everything in pod-02's inbox"*
> You: *"That's a recursive delete of `/app/workspace/inbox/`. Confirm?"*
> User: *"Yes"*
> You:
> ```bash
> tytus rm /app/workspace/inbox/ --pod 02 --recursive
> ```
> (Or narrower: `tytus rm /app/workspace/inbox/old-report.pdf --pod 02`)

**EN, size-exceeded:**

> User: *"Push this 2 GB mp4 to pod 2"*
> You (after the CLI rejects it): *"Refused — 100 MB ceiling. This is
> intentional; docker-exec base64 streaming is the wrong foundation for
> GB-scale files. The v0.7 Garage sprint ships a shared S3-backed
> filesystem that handles this. For now, options: (a) split the file,
> (b) wait for v0.7, (c) upload to the pod via a different path (e.g.
> scp if the pod has SSH enabled — it typically doesn't)."*

## Cross-CLI invocation (same command everywhere)

Because the skill routes to the `tytus` CLI binary on PATH, the
actual invocation is identical across every infected CLI:

| CLI | How to run |
|---|---|
| claude-code | Execute a `Bash` tool call with the `tytus …` command |
| gemini-cli | Same: run in shell |
| pi | Run in the Bash tool |
| opencode | Bash node |
| codex | `!shell tytus …` inline |
| qwen | Bash in terminal |
| vibe | Shell block |
| cursor | Terminal panel |

The only CLI-specific tweak is how to call the shell. The command
itself is identical.

If `tytus` is missing: instruct the user to install it.

```bash
curl -sSL https://raw.githubusercontent.com/traylinx/tytus-cli/main/install.sh | sh
```

Then `tytus login` and they're done.

## Troubleshooting cheat sheet

| Symptom | Cause | Fix |
|---|---|---|
| `Not logged in. Run: tytus login` | No credentials yet | `tytus login` first |
| `No pods. Run: tytus connect` | No pods allocated | `tytus connect` or `tytus setup` |
| `multiple pods connected ([...]); specify --pod NN` | Ambiguous target | Add `--pod NN` explicitly |
| `path outside /app/workspace/` | User tried to push to pod `/etc/` | Explain rootfs is RO; use `/app/workspace/` |
| `transfer exceeds 100 MB ceiling` | Deliberate | Point at the Garage sprint (see "Hard limits") |
| `remote path does not exist` | Bad pull path | `tytus ls --pod NN` to discover |

## Verification that this skill is active

```bash
skill_discover --match tytus-files
# Should return this SKILL.md's path on any infected CLI.
```

```bash
tytus --version
# Must be ≥ 0.4.0 to have the push/pull/ls/rm verbs.
```
