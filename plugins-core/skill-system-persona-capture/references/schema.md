# persona_registry.json schema v1

Location: `$MAKAKOO_HOME/config/persona_registry.json`.

Top-level keys:

- `version`: integer schema version, currently 1.
- `updated_at`: ISO-8601 UTC timestamp.
- `primary_persona`: primary Makakoo assistant identity mirrored from `config/persona.json`.
- `user`: preferred user identity.
- `companions`: map of slug -> companion/channel/mascot persona records.
- `channel_bindings`: map of channel/host -> active persona slug.
- `rules`: guardrails for primary rename and identity answers.
- `events`: bounded append-only list of capture/update events.

Companion record keys:

- `id`: slug.
- `name`: display name.
- `kind`: `channel-companion`, `mascot`, `agent`, `bot`, or free-form string.
- `roles`: list of role descriptions.
- `channels`: list of bound channels/hosts.
- `relationships`: list of `{kind, target, label}`.
- `notes`: short durable notes, no secrets.
- `created_at`, `updated_at`: ISO-8601 UTC timestamps.
