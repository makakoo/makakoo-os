# Walkthrough — Voice (Twilio) subagent quickstart

End-to-end recipe: stand up a Twilio Voice slot under Makakoo OS.
Push-to-talk model — the caller leaves a message, the slot processes
it, future v2.1 will play the reply back. ~20 minutes.

## What you'll have at the end

- A Twilio number routed to your `makakoo-mcp` instance
- Inbound calls trigger TwiML asking the caller to leave a message
- Recordings flow into your slot as inbound text frames (with the
  recording URL stamped on `raw_metadata`)
- A non-allowlisted caller hears a polite "this number isn't
  authorized" message

## Prerequisites

- Makakoo OS installed
- A Twilio account (a $15 trial works; production needs a paid plan)
- A purchased Twilio phone number with **Voice** capability
- A public HTTPS URL for `makakoo-mcp --http`

## 1. Provision the Twilio number

1. Twilio Console → **Phone Numbers → Active Numbers** → buy a
   number with Voice capability.
2. In the number's **Voice & Fax** config, set **A CALL COMES IN**:
   - Type: **Webhook**
   - URL: `https://YOUR.PUBLIC.URL/transport/<slot_uuid>/<transport_uuid>/webhook`
   - HTTP method: **POST**
3. Note your **Account SID** (`AC…`) and **Auth Token** from the
   console dashboard.

## 2. Stash the auth token

```bash
makakoo secret set agent/secretary/voice-main/auth_token
```

The auth token doubles as the X-Twilio-Signature HMAC key AND the
basic-auth password for fetching recordings — single secret, two
uses.

## 3. Create the slot

Edit `~/MAKAKOO/config/agents/secretary.toml`:

```toml
[[transport]]
id = "voice-main"
kind = "voice_twilio"

[transport.config]
account_sid           = "ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
auth_token_ref        = "agent/secretary/voice-main/auth_token"
allowed_caller_ids    = ["+34600000001"]   # E.164 with +
public_base_url       = "https://YOUR.PUBLIC.URL"
```

`public_base_url` is what Twilio signs (the full URL Makakoo built
for the recording-callback). It MUST match what Twilio sees, byte
for byte (incl. scheme + port if non-default).

## 4. Validate + start

```bash
makakoo agent validate secretary
# GET /2010-04-01/Accounts/{AccountSID}.json with basic-auth.

makakoo agent start secretary
```

## 5. Place a test call

Call your Twilio number from a phone in `allowed_caller_ids`.

You should hear: "Hello, please leave your message after the tone."

After the beep, leave a short message. Twilio uploads the recording
and POSTs the recording-completed webhook back to Makakoo. The slot
emits an inbound frame with:

- `text = "[recording RE-XXXXXXXX]"` (v1 stub STT — pluggable)
- `raw_metadata.recording_url = "https://api.twilio.com/.../Recordings/RE-X.wav"`
- `raw_metadata.twilio_call_sid = "CA-..."`

Verify in the audit log:

```bash
makakoo agent audit secretary --last 10
```

## v2.1 deferred items

- Real STT (SwitchAILocal `whisper-1`) replacing the stub
- TTS-to-`<Play>` so the bot speaks its reply
- Realtime media streaming (no record-then-respond round-trip)

The adapter shape is locked so these slot in without breaking the
TwiML state machine.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| 401 on every webhook | `auth_token` mismatch | Re-stash; restart slot |
| 401 with correct token | `public_base_url` doesn't match what Twilio sees | Confirm scheme + port |
| Caller hears "isn't authorized" | E.164 not in `allowed_caller_ids` | Add the caller |
| Recording fetch fails | basic-auth not getting the auth token | Check the Twilio Recording URL accessible with `curl -u AC...:AUTH https://api.twilio.com/...wav` |
