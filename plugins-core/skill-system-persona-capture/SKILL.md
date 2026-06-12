---
name: persona-capture
version: 0.1.0
description: Persist identity/name setup across Makakoo OS. Use when the user says "my name is...", "your name is...", "call me...", "call this bot/agent...", "introduce X to Y", asks to save a persona/name permanently, or when a CLI answers with its raw model/harness identity instead of the Makakoo persona registry.
entrypoint: src/persona_capture.py
allowed-tools: [bash]
category: system
tags: [persona, identity, names, infected-clis, makakoo]
---

# persona-capture

## Purpose

Persist names and identity bindings globally for Makakoo OS. This avoids random per-session `WHO.md` files and keeps every infected CLI aligned.

Use this skill when the user sets or corrects any identity:

- "my name is Sebastian"
- "your name is Donna"
- "call this Telegram bot Donna"
- "Olibia is the mascot"
- "Harvey is Donna's brother"
- "save this persona/name permanently"
- a CLI answers "I am MiniMax/Codex/Pi" when it should use Makakoo identity

## Storage model

- `$MAKAKOO_HOME/config/persona.json` = **primary persona only**. On Sebastian's install this stays `Harvey` unless Sebastian explicitly says to rename the primary Makakoo persona.
- `$MAKAKOO_HOME/config/persona_registry.json` = structured registry for user name, companions, mascots, bot/channel identities, and channel bindings.
- `$MAKAKOO_HOME/config/persona_context.md` = generated concise prompt fragment every CLI can read when identity matters.

Do **not** overwrite `persona.json` for companions like Donna or Olibia. Use the registry.

## Default action

When triggered, run the capture script with the user's exact sentence and the current channel/host if known:

```bash
makakoo skill skill-system-persona-capture capture \
  --text "<verbatim user sentence>" \
  --channel "<codex|claude|opencode|pi|telegram|vps-pi|unknown>" \
  --source "<short context>"
```

Fallback when `makakoo skill` is unavailable or when using the legacy Harvey dispatcher:

```bash
harvey run persona-capture capture \
  --text "<verbatim user sentence>" \
  --channel "<channel>" \
  --source "<short context>"

# Direct fallback:
python3 "$MAKAKOO_HOME/plugins/skill-system-persona-capture/src/persona_capture.py" capture \
  --text "<verbatim user sentence>" \
  --channel "<channel>" \
  --source "<short context>"
```

Then read or quote the script result. It writes the registry, regenerates `persona_context.md`, and records a small event.

## Manual operations

```bash
# Show registry summary
makakoo skill skill-system-persona-capture show

# Regenerate persona_context.md from registry
makakoo skill skill-system-persona-capture render

# Set/correct user name
makakoo skill skill-system-persona-capture set-user --name Sebastian --full-name "Sebastian Schkudlara"

# Add or update a companion/channel identity
makakoo skill skill-system-persona-capture add-companion \
  --name Donna \
  --kind channel-companion \
  --role "Makakoo VPS/Pi and Telegram messenger companion" \
  --channel vps-pi \
  --channel telegram-donna \
  --relationship "sibling:Harvey:Harvey is Donna's brother"

# Bind a host/channel to a companion persona
makakoo skill skill-system-persona-capture bind-channel --channel vps-pi --persona Donna
```

## Hard rules

1. Primary persona rename needs explicit primary intent. Phrases like "your name is Donna" inside a channel/CLI session create a companion/channel binding by default; they do not rename Harvey.
2. Never save tokens, API keys, addresses, passwords, or private secrets in persona registry.
3. Keep generated `persona_context.md` short. It is prompt context, not a diary.
4. If ambiguous whether the user means primary Harvey or a channel companion, ask one short clarification before writing.
5. After meaningful identity changes, run `makakoo sync` if available and log to the Brain journal.

## Current Sebastian convention

- Primary persona: Harvey.
- User: Sebastian / Sebastian Schkudlara.
- Donna: companion/channel identity for Makakoo VPS Pi and Telegram messenger context; Harvey is her brother in the user-facing mythology.
- Olibia: guardian owl mascot; protective, encouraging, terse.
