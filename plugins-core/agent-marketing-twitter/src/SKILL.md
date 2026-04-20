---
name: marketing-twitter
description: Generate AND post Twitter/X launch threads using the structure validated on the lope v0.3.0 launch — 10-14 tweets, each under 280 chars, hook → demo → why → features → CTA.
---

# marketing-twitter

## CONTRACT — read this first

**This agent DOES:**
- Generate text threads from a campaign brief (`generate_thread`)
- Count characters per tweet and refuse to write a thread with any tweet over 280 (`count_chars`)
- Parse a thread file into individual tweets (`parse`)
- Preflight-check X API credentials and tweepy availability (`preflight`)
- Upload media + post threads via X API v2 with reply chaining (`post_thread`)

**This agent DOES NOT:**
- Generate images (use `marketing-twitter` text + a separate image-generation step)
- Schedule posts (no built-in scheduler — typefully/buffer integration is future work)
- Handle DMs, polls, spaces, or quote tweets
- Retry on rate limit — if X throttles, the call fails loudly with the raw error

**This agent REQUIRES:**
- For `generate_thread`: switchAILocal reachable at `OPENAI_BASE_URL` (default `http://localhost:18080/v1`)
- For `post_thread`: OAuth 1.0a credentials, searched in order:
    1. `$MAKAKOO_HOME/.env` (primary)
    2. `$HARVEY_HOME/.env` (compat)
    3. `~/.config/x-cli/.env` (x-cli / xitter skill — **currently in use**)
    - `X_API_KEY`, `X_API_SECRET` (from developer.x.com app)
    - `X_ACCESS_TOKEN`, `X_ACCESS_SECRET` (OAuth 1.0a user context)
    - `X_ACCESS_TOKEN_SECRET` also accepted (auto-mapped to `X_ACCESS_SECRET`)
    - App must have **Read and Write** permission enabled
- For `post_thread` with images: `tweepy` installed (`pip3 install tweepy`)

**On failure this agent:**
- Prints exactly what is missing or wrong
- Exits with a non-zero code
- **NEVER invents workarounds, NEVER claims the X API "doesn't support" a thing that it does, NEVER silently posts a partial thread without telling you**

If this agent tells you something is impossible, verify it against https://developer.x.com/en/docs. The X API v2 supports: text tweets, image/video upload (v1.1), media attachment to tweets (v2), reply chaining, threading. All are implemented here.

Reusable Twitter thread generator for product launches. Produces a numbered thread where every tweet is under 280 characters, with explicit role labels for copy-paste into X, Typefully, or any scheduler.

## Usage

```bash
harvey skill run marketing-twitter generate_thread \
  --brief ~/MAKAKOO/marketing/<product>/brief.md \
  --product lope \
  --tweet-count 12 \
  --style fact \
  --out ~/MAKAKOO/marketing/<product>/plan/TWITTER_THREAD.md
```

## Thread structure (frozen from lope launch)

Every launch thread is 10-14 tweets in this order. Each slot has a specific job.

| # | Slot | Role | Example from lope |
|---|---|---|---|
| 1 | `hook` | The one-line promise that makes people click "show more" | "Shipped: Lope. Any AI CLI implements. Any AI CLI validates. Majority vote. No single-model blindspot." |
| 2 | `demo` | What it actually looks like — paste-friendly code or prose | `/lope-negotiate "Add JWT auth"` block |
| 3 | `why` | The problem nobody talks about — the insight that sells the product | "When the same model writes AND reviews, it has correlated failures." |
| 4 | `killer-feature-1` | One feature with a concrete number or name | "12 built-in validators out of the box" |
| 5 | `killer-feature-2` | Second feature, different category | "Two-stage review: spec, then quality" |
| 6 | `expansion` | Not just X — also Y (broaden perceived market) | "Not just code. Business docs. Research protocols." |
| 7 | `dogfood` | Proof + receipt — numbers, tests, bugs shipped | "81 tests. 9 bugs caught in one session." |
| 8 | `lore-or-easter-egg` | The weird detail that gets screenshots | "The lion face in every log" |
| 9 | `origin` | Optional — why it exists, personal | "How a failing sprint made me build this" |
| 10 | `install` | Copy-paste install line | "Install: read github.com/traylinx/lope/INSTALL.md" |
| 11 | `cta-1` | Ask — star, break, open issues | "Star it. Break it. Open issues." |
| 12 | `cta-2` | Thread end + link | "github.com/traylinx/lope" |
| 13-14 | `overflow` | Optional extra features or follow-up threads | — |

## Hard rules

- **Every tweet ≤ 280 characters** — verify count after generation, regenerate any that exceed
- **Numbered format**: `**Tweet N (role)**` header, blank line, tweet body, blank line, `---`
- **No emojis unless requested** — Harvey tone default, emojis dilute precision
- **Max 1 URL in the thread** — put it in the install or cta-2 tweet, nowhere else
- **No hashtags in body tweets** — hashtags only in the final tweet, max 3
- **Hook must stand alone** — if it got retweeted without the thread, would it still make sense?

## Style modifiers

| Style | Hook pattern | Tone |
|---|---|---|
| `fact` (default) | "Shipped: X. Here's what it does." | Direct, confident, no buildup |
| `question` | "Why does every AI code review rubber-stamp the bug it just wrote?" | Provocative, makes reader think |
| `provocation` | "Your AI code reviewer has the same blindspot as the AI that wrote the code." | Contrarian, names the enemy |

## Reference example

`examples/lope-twitter-thread.md` — the actual shipped 14-tweet lope launch thread, including char counts. Every tweet was measured and is under 280. Use as few-shot anchor.

## How today's thread was generated

One prompt to switchAILocal with:
1. `SKILL.md` (this file) as system context
2. `examples/lope-twitter-thread.md` as few-shot anchor
3. The campaign brief as user context
4. Slot-by-slot instructions with the table above

Output parsed into 14 individual tweet blocks, each validated for ≤280 chars. Tweets over the limit were regenerated tweet-by-tweet until clean. Whole flow ~5 minutes end-to-end.

## Character counting

```bash
# Verify a draft thread
bash run.sh count_chars --file TWITTER_THREAD.md
# Returns: tweet 1: 187/280 ✓ / tweet 2: 264/280 ✓ / tweet 3: 301/280 ✗ OVER
```
