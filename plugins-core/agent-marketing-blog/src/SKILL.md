---
name: marketing-blog
description: Generate Jekyll blog posts for jevvellabsblog using the structure Harvey's lope launch campaign validated on 2026-04-13.
---

# marketing-blog

## CONTRACT — read this first

**This agent DOES:**
- Generate Jekyll blog posts from a campaign brief (`generate_post`, `generate_campaign`)
- Preflight-check switchAILocal + jevvellabsblog repo presence (`preflight`)
- Stage posts to the jevvellabsblog `_posts/` directory with Jekyll frontmatter and date-slug naming (`publish_flow`)

**This agent DOES NOT:**
- Commit or push to jevvellabsblog automatically (it prints the exact git commands for you to run)
- Publish directly to Medium, dev.to, Substack, or anywhere other than jevvellabsblog
- Generate images or diagrams (use a separate image-generation step)
- Validate Jekyll syntax or build the site — it only writes the markdown

**This agent REQUIRES:**
- For `generate_*`: switchAILocal reachable at `OPENAI_BASE_URL` (default `http://localhost:18080/v1`)
- For `publish_flow`: `JEVVELLABSBLOG_REPO` env var (or default `~/code/jevvellabsblog`) pointing at a writable git clone of the blog repo

**On failure this agent:**
- Prints exactly which file, env var, or directory is missing
- Exits non-zero
- **NEVER fabricates a successful publish. NEVER claims a post was shipped without actually staging the file. The `publish_flow` subcommand prints the exact git commands — if you didn't run them yourself, the post is not live.**

Reusable launch-post generator. Takes a campaign brief + audience + angle and produces a Jekyll-compatible markdown file ready to drop into `jevvellabsblog/_posts/`. Structure and tone are frozen from the lope v0.3.0 / lope-business campaigns — both shipped and battle-tested.

## Usage

```bash
# Single post for a specific audience
harvey skill run marketing-blog generate_post \
  --brief ~/MAKAKOO/marketing/<product>/brief.md \
  --audience dev \
  --angle launch \
  --product lope \
  --out ~/MAKAKOO/marketing/<product>/blog/01-launch.md

# Full campaign (one post per audience listed in brief)
harvey skill run marketing-blog generate_campaign \
  --brief ~/MAKAKOO/marketing/<product>/brief.md \
  --product lope \
  --out-dir ~/MAKAKOO/marketing/<product>/blog/
```

Or from any CLI:
```bash
bash ~/MAKAKOO/agents/marketing-blog/run.sh generate_post \
  --brief BRIEF.md --audience marketing-ops --angle use-case --product lope
```

## The structure (frozen from lope launch)

Every post follows this 5-section skeleton:

1. **Headline** — product + the one-line promise (e.g., *"Introducing Lope: Any AI CLI Implements. Any AI CLI Validates."*)
2. **ASCII art block or visual hook** — optional but encouraged, distinctive identity element
3. **The problem** — the correlated-failure insight OR the status-quo pain, 1-3 paragraphs, specific and concrete
4. **What it does** — walkthrough with a real code/prose example showing input → output
5. **Proof + install** — a dogfood receipt (numbers, commits, test counts) then a copy-paste install line

## Tone rules

- Sharp, specific, zero sycophancy
- Name the enemy (correlated failure, single-model blindspot, drift, etc.)
- Show don't tell — every claim backed by a snippet, number, or commit hash
- No "in today's world" filler, no "leverage synergies"
- Humor through precision, not jokes
- Copy-pasteable: install line must actually work

## Audience modifiers

| Audience | Angle shift | Vocabulary |
|---|---|---|
| `dev` | Technical insight + code snippet | git, commit, diff, CI, regression |
| `marketing-ops` | KPI gaps, budget accuracy | MQL/SQL, CAC, LTV, attribution, campaign brief |
| `finance` | Close checklists, forecast gaps | FP&A, variance, accrual, control, tie-out |
| `consulting` | SOW scope, client risk | engagement, deliverable, scope creep, margin |
| `legal` | Policy gaps, compliance | GRC, SOC 2, clause, precedent, exposure |
| `research` | Methodology, reproducibility | PRISMA, validity, sampling, reproducible |
| `general` | Productivity + correlated failure | work, document, review, ship |

## Reference examples

- `examples/lope-launch-announcement.md` — the developer-audience launch post (the original)
- `examples/lope-beyond-code.md` — the business-expansion post (non-developer audience)
- `examples/lope-origin-story.md` — the origin narrative post

## How today's campaign was generated (2026-04-13)

1. Wrote `marketing/lope-business/campaign-brief.md` with 5 audience personas + pain/lope-angle mapping
2. For each audience ran a prompt: `"Using BRIEF.md and the structure in SKILL.md, write a <angle> post for <audience>. Target <word_count> words. Name the enemy, show one proof snippet, end with install line."`
3. Saved to `marketing/<product>/blog/NN-title.md`
4. Copied blog posts to separate `jevvellabsblog` git repo (Jekyll `_posts/` dir), renamed with date prefix (`YYYY-MM-DD-slug.md`), added Jekyll frontmatter, committed to main, pushed
5. Non-blog assets stayed in `~/MAKAKOO/marketing/<product>/`

lope codex adapter was broken (--quiet flag), so the campaign was drafted directly from Claude Code with Harvey tone. Agent encodes that workflow as the default path.

## Publishing flow (jevvellabsblog)

```bash
# 1. Generate post in the HARVEY marketing tree
bash run.sh generate_post --brief BRIEF.md --audience dev --product <name> --out /tmp/post.md

# 2. Transform for Jekyll (add date prefix + frontmatter)
# Jekyll frontmatter template is at templates/jekyll-frontmatter.md
SLUG=$(head -1 /tmp/post.md | sed 's/# //; s/[^a-z0-9]/-/gi; s/--*/-/g' | tr A-Z a-z)
DATE=$(date +%Y-%m-%d)
cp /tmp/post.md ~/code/jevvellabsblog/_posts/$DATE-$SLUG.md
# prepend Jekyll frontmatter manually or via helper

# 3. Commit + push in the blog repo
cd ~/code/jevvellabsblog
git add _posts/$DATE-$SLUG.md
git commit -m "blog: $SLUG"
git push origin main
```

Full flow is automated in `run.sh publish_flow` if all paths resolve.
