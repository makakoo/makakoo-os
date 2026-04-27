---
name: marketing-linkedin
description: Generate AND post LinkedIn launch content using the 10-theme wheel validated on the lope v0.3.0 campaign.
---

# marketing-linkedin

## CONTRACT — read this first

**This agent DOES:**
- Generate posts from a campaign brief (`generate_post`, `generate_wheel`)
- Preflight-check LinkedIn API credentials (`preflight`)
- Post text updates via LinkedIn UGC Posts API (`post_text`)

**This agent DOES NOT:**
- Upload images (LinkedIn v2 asset upload is a 2-step flow — not implemented here yet; if you need it, see https://learn.microsoft.com/en-us/linkedin/marketing/integrations/community-management/shares/vector-asset-api)
- Post to LinkedIn Company Pages (only personal profiles via `urn:li:person`)
- Post polls, documents, events, articles
- Refresh expired tokens (LinkedIn 60-day token expiry requires partner status)
- Read your LinkedIn feed, comments, or connections

**This agent REQUIRES:**
- For `generate_*`: switchAILocal reachable at `OPENAI_BASE_URL` (default `http://localhost:18080/v1`)
- For `post_text`: `LINKEDIN_ACCESS_TOKEN` in `~/MAKAKOO/.env` — OAuth 2.0 user access token with `w_member_social` scope
- Token acquisition:
    1. Create app at https://www.linkedin.com/developers/apps
    2. Request "Share on LinkedIn" product → grants `w_member_social`
    3. Run OAuth 2.0 authorization code flow (3-legged)
    4. Exchange code → access_token at `/oauth/v2/accessToken`
    5. Token expires in 60 days

**On failure this agent:**
- Prints the exact LinkedIn API response body (raw JSON)
- Exits non-zero
- **NEVER claims LinkedIn "blocks" something without showing the real HTTP error. NEVER says "try manual posting" as a silent fallback — only suggests it when the actual API call has failed with a specific reason.**

If this agent says text posting is impossible, the error will include the HTTP status code and response body. Do not trust any agent that says "LinkedIn API doesn't allow X" without showing the raw error.

Reusable LinkedIn post generator. Takes a campaign brief + theme and produces a 150-300 word post ready to paste into LinkedIn. Can also generate the full 10-post wheel for a multi-day launch cadence.

## Usage

```bash
# Single post
harvey skill run marketing-linkedin generate_post \
  --brief ~/MAKAKOO/marketing/<product>/brief.md \
  --theme launch-day \
  --product lope \
  --out ~/MAKAKOO/marketing/<product>/linkedin/01-launch-day.md

# Full 10-post wheel (one post per canonical theme)
harvey skill run marketing-linkedin generate_wheel \
  --brief ~/MAKAKOO/marketing/<product>/brief.md \
  --product lope \
  --out-dir ~/MAKAKOO/marketing/<product>/linkedin/
```

## The 10-post wheel (frozen from lope launch)

Every LinkedIn launch campaign generates exactly ten posts, one per theme. The themes are ordered for a two-week cadence (one post every 1-2 days), each hitting a different cognitive angle so the audience never feels repetition.

| # | Theme | Cognitive angle | Example title (lope) |
|---|---|---|---|
| 01 | `launch-day` | Announcement + what ships | "Shipped: Lope. Open source, MIT." |
| 02 | `single-insight` | The sharpest idea in the product, standalone | "Correlated failure: the blindspot AI review can't see" |
| 03 | `killer-feature` | One feature that embodies the product | "Mistral Vibe support — why the least obvious model matters" |
| 04 | `personality` | The humor, design, easter egg, or stance | "Caveman mode: saving 63% of output tokens by saying less" |
| 05 | `use-case-expansion` | Not just X — also Y and Z | "It's not just code. Lope validates business docs and research protocols too." |
| 06 | `lore` | The weird detail that makes people look twice | "The lion-face easter egg in every sprint" |
| 07 | `origin` | Why this exists, personal story | "How a failing sprint doc made me build lope" |
| 08 | `dogfood-receipt` | Proof: numbers, commits, test counts | "81 tests pass. 9 bugs shipped + fixed in 6 hours. The receipt." |
| 09 | `install-cta` | Zero friction copy-paste for doers | "One line to install lope in any AI CLI" |
| 10 | `community-call` | Ask for contribution or validation | "Looking for validators: which AI CLI should we add next?" |

## Format rules (non-negotiable)

- **150-300 words** — LinkedIn truncates at ~200, so first 2 sentences must carry the hook
- **First line is the hook** — blank line after, then the body
- **Concrete > abstract** — every claim needs a number, a name, or a snippet
- **Zero sycophancy** — no "Excited to share", no "Thrilled to announce", no "I'm humbled"
- **End with a CTA + hashtags** — one install line OR one question + 4-5 hashtags max
- **One link max** — LinkedIn throttles multi-link posts; put the repo URL in the body, nothing else

## Tone calibration

Same as marketing-blog: sharp, specific, "senior staff engineer with a decade of partnership" voice. Humor through precision, not jokes. Never start with "In today's world of AI".

## Reference examples

Every post in `examples/` shipped on the 2026-04-13 lope v0.3.0 launch and was validated against the tone rules above. Use them as few-shot anchors.

- `examples/01-launch-day.md` — theme: launch-day
- `examples/02-single-model-blindspot.md` — theme: single-insight
- `examples/04-caveman-mode.md` — theme: personality
- `examples/08-dogfood-receipt.md` — theme: dogfood-receipt

## How today's campaign was generated

Ten posts produced in parallel from one brief (`marketing/lope/plan/*`). Each prompt seeded the drafter with the brief + the target theme + one reference example from an earlier post in the wheel. Result: 10 posts, varied cognitive angles, consistent voice. Entire set took ~15 minutes end to end.

For the business audience variant (`marketing/lope-business/linkedin-posts.md`) the same 10-theme wheel mapped to finance/consulting/marketing-ops personas — same structure, different vocabulary. The wheel is audience-agnostic; only the examples and brief change.
