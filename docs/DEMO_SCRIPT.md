# v0.1 demo script

Shoot target: 3–4 minutes, single continuous screen capture, no cuts. Optional B-roll for the closing montage.

## Setup

- Fresh macOS user account (or VM). `~/MAKAKOO/` doesn't exist yet.
- Terminal at a readable font size (18pt minimum — phone viewers).
- `asciinema rec` for a lossless recording; overlay a cursor + keystroke display if the recording tool supports it (Keycastr on macOS).
- Two terminal windows pre-opened side by side: **Claude Code** in the left pane, **Gemini CLI** in the right. Both empty sessions.

## Beat sheet

### 00:00 — cold open (15s)

Static frame. Text on screen:

> "AI agents don't remember. Every session starts from zero. Every CLI has its own fishbowl."
>
> Makakoo OS fixes this.

Cut to terminal, prompt sitting at `$`.

### 00:15 — install (45s)

Voice-over, no text:

```sh
$ curl -fsSL https://makakoo.com/install | sh
```

Let the install ticker play. After it finishes:

```sh
$ makakoo install
```

Umbrella scrolls through detection + distro install + daemon install + infect. Pause on the "install complete" summary.

### 01:00 — write something in Claude (30s)

Switch to the left pane (Claude Code). Type into Claude:

> I'm researching the trade-offs between pgvector and Qdrant for a RAG pipeline. Help me jot the decision. I'll pick Qdrant because Gemini Embedding 2 is 3072-dim and pgvector caps at 2000.

Let Claude respond. Then:

> Journal this decision in my Brain please.

Claude writes to `~/MAKAKOO/data/Brain/journals/<today>.md`.

### 01:30 — Gemini sees it (30s)

Switch to the right pane (Gemini CLI). No context-reset ceremony. Type:

> What did I decide about vector databases today?

Gemini reads the Brain, surfaces the Qdrant vs pgvector note. On-screen text overlay:

> "Same Brain. Different body."

### 02:00 — show the plugin system (30s)

Back to the shell:

```sh
$ makakoo plugin list
```

Scroll the 38 plugins. Narration:

> "Every capability is a plugin manifest. Skills, watchdogs, mascots, agents. Capability-sandboxed, Unix-socket or named-pipe IPC, audit-logged."

```sh
$ makakoo plugin info skill-research-arxiv
$ makakoo sancho status
```

Show the "N registered task(s) (8 native + M manifest)" line.

### 02:30 — write a plugin in 20 seconds (45s)

```sh
$ python3 scripts/migrate_skill.py research my-new-skill
$ cat plugins-core/skill-research-my-new-skill/plugin.toml
```

Show the generated manifest. Narration:

> "The migration IS the manifest. Twenty lines of TOML wraps any Python script you already have. Capabilities you declare. Sandbox you get."

### 03:15 — close (30s)

Full-screen text:

> Makakoo OS v0.1.0
> MIT · local-first · no VC · no telemetry
> Many bodies. One mind.
>
> github.com/makakoo/makakoo-os

Owl mascot in the corner. Fade.

## Post-production notes

- Export two versions: 16:9 (YouTube / blog embed) and 9:16 vertical (social, crop the terminal to the active pane).
- Captions in English. Subtitle file separate so other contributors can add translations.
- Thumbnail candidate: the moment Gemini recalls the decision Claude wrote 45 seconds earlier. Owl in the bottom-right.

## What this demo does NOT show (on purpose)

- The actual one-liner URL is placeholder until DNS lands. Film the `curl` step from a pre-cached tarball and edit the URL banner in post if needed.
- Capability-denied errors — they're worth a separate 90-second demo later.
- Windows + Linux smokes — cover those in a sibling "three platforms, one bootstrap" montage after v0.1 lands.
- Signing dialogs — v0.1 ships unsigned; the first-run Gatekeeper dialog is not a launch talking point.
