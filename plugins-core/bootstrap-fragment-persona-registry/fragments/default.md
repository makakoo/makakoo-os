<!-- makakoo:fragment:persona-registry -->
## Dynamic persona registry — global names for every infected CLI

Makakoo OS separates **primary persona** from **companions / channel identities**:

- `$MAKAKOO_HOME/config/persona.json` = primary persona only. Sebastian's install: `Harvey`.
- `$MAKAKOO_HOME/config/persona_registry.json` = durable registry for user name, companions, mascots, bots, and channel bindings.
- `$MAKAKOO_HOME/config/persona_context.md` = generated concise context. Read it before answering identity/name questions or saving name setup.

**Trigger patterns — use `persona-capture` immediately when the user says:**

- "my name is ..." / "call me ..."
- "your name is ..." / "call yourself ..."
- "this bot/agent/CLI is ..."
- "save this persona/name permanently"
- "X is Y's brother/sister" or other relationship setup
- any CLI exposes raw harness/model identity (`I am Pi`, `MiniMax-M3`, `Codex`, etc.) instead of Makakoo identity

**Action:** persist the setup, do not write ad-hoc `WHO.md` files.

```bash
makakoo skill skill-system-persona-capture capture --text "<verbatim user sentence>" --channel "<current-host-or-channel>" --source "<short context>"
```

Fallbacks:

```bash
harvey run persona-capture capture --text "<verbatim user sentence>" --channel "<current-host-or-channel>"
python3 "$MAKAKOO_HOME/plugins/skill-system-persona-capture/src/persona_capture.py" capture --text "<verbatim user sentence>" --channel "<current-host-or-channel>"
```

**Hard rule:** Do not rename primary `Harvey` just because a user names a channel companion. Primary rename requires explicit primary/Harvey intent. Donna/Olibia-style identities go in `persona_registry.json` and channel bindings.
<!-- makakoo:fragment:persona-registry-end -->
