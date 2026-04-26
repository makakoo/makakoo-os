# Walkthrough — WhatsApp Business Cloud API subagent

End-to-end recipe: stand up a WhatsApp-backed subagent slot under
Makakoo OS using Meta's Cloud API. ~30 minutes (most of it on Meta's
side filling out forms).

## What you'll have at the end

- A WhatsApp Business app + phone number registered with Meta
- Inbound messages from approved senders flowing into your slot
- Outbound replies via `POST /v18.0/{phone_number_id}/messages`
- Inbound media → polite "I can only read text" auto-reply

## Prerequisites

- Makakoo OS installed
- A Meta developer account with a Business Verified org (or the
  test number Meta gives you while pending verification — limited to
  5 destinations and 1000 free messages/month)
- A public HTTPS URL pointing at your `makakoo-mcp --http` instance
  (Cloudflare Tunnel, ngrok, or your own reverse proxy will do)

## 1. Provision the WhatsApp app

1. Go to <https://developers.facebook.com/apps>, **Create App** of
   type **Business**.
2. Add the **WhatsApp** product.
3. Note the **Phone Number ID** and **Temporary Access Token**
   (24-hour token; for production, generate a System User token of
   60-day or no-expiry shape).
4. Set the **Webhook URL** in the WhatsApp configuration tab to:
   ```
   https://YOUR.PUBLIC.URL/transport/<slot_uuid>/<transport_uuid>/webhook
   ```
   You'll get the UUIDs from `makakoo agent show <slot> --json` after
   step 3 below.
5. Set the **Verify Token** to a string of your choice (e.g. a 32-char
   random hex). Stash it locally — Makakoo's adapter checks it on the
   GET handshake.
6. Subscribe to the `messages` field at minimum.

## 2. Stash credentials

```bash
# Cloud API access token (Bearer for outbound)
makakoo secret set agent/secretary/whatsapp-main/access_token

# Webhook subscription verify token
makakoo secret set agent/secretary/whatsapp-main/verify_token

# App secret used to verify X-Hub-Signature-256
makakoo secret set agent/secretary/whatsapp-main/app_secret
```

## 3. Create the slot

Edit `~/MAKAKOO/config/agents/secretary.toml` (after `makakoo agent
create secretary` scaffolds the base):

```toml
[[transport]]
id = "whatsapp-main"
kind = "whatsapp"
secret_ref = "agent/secretary/whatsapp-main/access_token"

[transport.config]
phone_number_id      = "1234567890123456"
graph_version        = "v18.0"
verify_token_ref     = "agent/secretary/whatsapp-main/verify_token"
app_secret_ref       = "agent/secretary/whatsapp-main/app_secret"
allowed_wa_ids       = ["34600000001"]    # E.164 without +
```

`allowed_wa_ids` is least-privilege deny-all when empty.

## 4. Validate + start

```bash
makakoo agent validate secretary
# GET /v18.0/{phone_number_id} — confirms token + that it controls
# the configured number.

makakoo agent start secretary
```

## 5. Verify the webhook

In the Meta WhatsApp configuration tab, click **Verify and Save**
on the webhook URL. Meta sends a GET with `hub.mode=subscribe`,
`hub.verify_token=YOUR_TOKEN`, `hub.challenge=xxx`. Makakoo's
handler echoes the challenge if the verify_token matches.

If validation fails (red error), check the `agent audit` log:

```bash
makakoo agent audit secretary --kind webhook_invalid_signature --last 5
makakoo agent audit secretary --kind webhook_bad_request --last 5
```

## 6. Send a test message

From a phone whose `wa_id` is on the allowlist, send a WhatsApp
message to the number you registered. Within seconds the slot's
gateway will receive an inbound frame and reply.

Try sending a photo or audio clip → the adapter responds with the
locked drop-reply: "Thanks — I can only read text messages right
now. Please re-send as text."

## Hard rules

- **Production access token must be a System User token.** The 24-h
  test token will silently fail outbound after expiry.
- **`app_secret` is the X-Hub-Signature-256 key.** Rotating it requires
  updating the App Settings page AND `makakoo secret set ...` in lockstep.
- **Allowed senders only.** v1's allowlist is a hard gate — there is
  no "log first, allow later" mode.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Webhook 401 on every POST | `app_secret` mismatch | Re-stash the secret; restart slot |
| Webhook 401 on the GET handshake | `verify_token` mismatch | Confirm both sides use the same string |
| Outbound 400 "Recipient not in allowlist" | Test number caps | Add the recipient in Meta's WA tester UI |
| Inbound dropped silently | `allowed_wa_ids` mismatch | Add the sender's E.164 (no `+`) |
