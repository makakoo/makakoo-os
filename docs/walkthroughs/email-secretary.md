# Walkthrough — Email secretary subagent

**Status:** Email transport adapter ships in v2.1. This walkthrough
documents the locked behavior + slot.toml shape so production
deployments can plan around it.

## Why email is its own phase

Unlike webhook-based transports (Slack Events / WhatsApp / Twilio)
or socket-based ones (Telegram polling, Discord/Slack WS), email
needs a long-lived IMAP IDLE listener to push inbound messages
without polling. The Makakoo Email adapter wraps `async-imap` for
the IDLE loop and `lettre` for SMTP outbound.

## Locked schema (v2.1)

```toml
[[transport]]
id = "email-main"
kind = "email"
secret_ref = "agent/secretary/email-main/oauth2_refresh_token"

[transport.config]
account_id        = "secretary@example.com"   # full mailbox address
auth_mode         = "oauth2"                   # OR "app_password"
imap_server       = "imap.gmail.com"
imap_port         = 993
smtp_server       = "smtp.gmail.com"
smtp_port         = 465
allowed_senders   = ["alice@example.com"]
support_thread    = true                       # populate Message-ID threading
```

`auth_mode = "oauth2"` is mandatory for Gmail (Google enforces it).
`auth_mode = "app_password"` is acceptable for IMAP servers that
still allow it but is documented as weaker (the adapter logs a WARN
on every `agent start`).

## Locked behavior

- **IMAP IDLE reconnect:** cap 25 minutes (RFC 2177 limit), heartbeat
  NOOP every 5 minutes.
- **STARTTLS:** required. Plain (non-TLS) IMAP/SMTP rejected by
  `validate`.
- **OAuth2 refresh:** automatic 60 seconds before token expiry.
- **Threading:** outbound replies stamp `In-Reply-To` and `References`
  from the inbound `Message-ID`. `conversation_id = root Message-ID`.
- **Reply parsing:** custom quote-line stripper handles common
  client formats (Gmail/Outlook/Apple Mail) — no Python deps.
- **`raw_metadata.body_raw`:** always preserves the full unparsed
  body for forensics.

## When v2.1 lands

Run:

```bash
makakoo secret set agent/secretary/email-main/oauth2_refresh_token
makakoo agent validate secretary
makakoo agent start secretary
```

Send an email from `alice@example.com` to `secretary@example.com`,
expect a reply within ~30s. Reply to that thread → conversation_id
is preserved.

## Hard rules

- **Plain IMAP/SMTP rejected by validate.** Use STARTTLS or TLS.
- **OAuth2 mandatory for Gmail.** Switch to a different provider if
  you really need app passwords.
- **Allowlist deny-all.** `allowed_senders = []` drops every inbound.
