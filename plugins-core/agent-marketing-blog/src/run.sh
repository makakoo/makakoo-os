#!/usr/bin/env bash
# marketing-blog — Jekyll blog post generator
# Wraps switchAILocal with the frozen lope-launch structure.
#
# Usage:
#   run.sh generate_post --brief BRIEF.md --audience dev --angle launch --product lope --out /path/to/out.md
#   run.sh generate_campaign --brief BRIEF.md --product lope --out-dir /path/to/blog/
#
# Inputs flow through switchAILocal via the OPENAI_* env vars. Load from
# HARVEY's .env or source a provider file before calling.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"

# Default switchAILocal endpoint
OPENAI_BASE_URL="${OPENAI_BASE_URL:-http://localhost:18080/v1}"
OPENAI_API_KEY="${OPENAI_API_KEY:-$(grep -E "^LLM_API_KEY=" "$HARVEY_HOME/.env" 2>/dev/null | cut -d= -f2- || echo sk-test-123)}"
MODEL="${MARKETING_BLOG_MODEL:-minimax:MiniMax-M2.7}"

SKILL_MD="$SCRIPT_DIR/SKILL.md"
TEMPLATES_DIR="$SCRIPT_DIR/templates"
EXAMPLES_DIR="$SCRIPT_DIR/examples"

usage() {
  cat <<EOF
Usage:
  GENERATE:
    $0 generate_post --brief BRIEF.md --audience AUD --angle ANGLE --product NAME --out OUT.md
    $0 generate_campaign --brief BRIEF.md --product NAME --out-dir DIR/

  PUBLISH:
    $0 preflight                                          # check deps + repo + switchAILocal
    $0 publish_flow --post OUT.md --slug SLUG [--blog-repo DIR]

Audiences: dev, marketing-ops, finance, consulting, legal, research, general
Angles:    launch, deep-dive, origin, feature-spotlight, use-case

Env vars:
  JEVVELLABSBLOG_REPO  Path to jevvellabsblog git clone (default: ~/code/jevvellabsblog)
  OPENAI_BASE_URL      switchAILocal endpoint (default: http://localhost:18080/v1)
EOF
  exit 1
}

CMD="${1:-}"; shift || usage

# --- flag parsing ---
BRIEF=""; AUDIENCE="dev"; ANGLE="launch"; PRODUCT=""; OUT=""; OUT_DIR=""; POST=""; SLUG=""
BLOG_REPO="${JEVVELLABSBLOG_REPO:-$HOME/code/jevvellabsblog}"
WORD_COUNT="1200"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --brief) BRIEF="$2"; shift 2;;
    --audience) AUDIENCE="$2"; shift 2;;
    --angle) ANGLE="$2"; shift 2;;
    --product) PRODUCT="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --out-dir) OUT_DIR="$2"; shift 2;;
    --post) POST="$2"; shift 2;;
    --slug) SLUG="$2"; shift 2;;
    --blog-repo) BLOG_REPO="$2"; shift 2;;
    --word-count) WORD_COUNT="$2"; shift 2;;
    *) echo "unknown flag: $1"; usage;;
  esac
done

call_llm() {
  local system_prompt="$1"
  local user_prompt="$2"
  python3 - <<PY
import json, os, sys, urllib.request
base = os.environ["OPENAI_BASE_URL"].rstrip("/")
key = os.environ["OPENAI_API_KEY"]
model = os.environ["MODEL"]
payload = {
    "model": model,
    "messages": [
        {"role": "system", "content": """$system_prompt"""},
        {"role": "user", "content": """$user_prompt"""},
    ],
    "temperature": 0.7,
    "max_tokens": 4000,
}
req = urllib.request.Request(
    f"{base}/chat/completions",
    data=json.dumps(payload).encode(),
    headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=300) as resp:
    body = json.loads(resp.read())
print(body["choices"][0]["message"]["content"])
PY
}

build_system_prompt() {
  cat "$SKILL_MD"
  echo ""
  echo "## Reference example (frozen — match this quality bar)"
  cat "$EXAMPLES_DIR/lope-launch-announcement.md" 2>/dev/null || true
}

generate_post() {
  [[ -z "$BRIEF" || -z "$PRODUCT" || -z "$OUT" ]] && usage
  [[ ! -f "$BRIEF" ]] && { echo "brief not found: $BRIEF"; exit 2; }
  local system_prompt user_prompt brief_content
  brief_content="$(cat "$BRIEF")"
  system_prompt="$(build_system_prompt)"
  user_prompt="Campaign brief:
$brief_content

Target audience: $AUDIENCE
Post angle: $ANGLE
Product: $PRODUCT
Target word count: $WORD_COUNT

Write a Jekyll blog post matching the structure and tone in the system prompt. Output pure markdown — no preamble, no explanation, no JSON wrapper. Start with the '# Title' line. Use the specific audience vocabulary listed in the skill doc. End with a working install line."
  echo "Generating blog post: $PRODUCT / $AUDIENCE / $ANGLE → $OUT" >&2
  OPENAI_BASE_URL="$OPENAI_BASE_URL" OPENAI_API_KEY="$OPENAI_API_KEY" MODEL="$MODEL" \
    call_llm "$system_prompt" "$user_prompt" > "$OUT"
  echo "✓ wrote $OUT ($(wc -w < "$OUT") words)" >&2
}

generate_campaign() {
  [[ -z "$BRIEF" || -z "$PRODUCT" || -z "$OUT_DIR" ]] && usage
  mkdir -p "$OUT_DIR"
  local i=1
  for aud in dev marketing-ops finance consulting research; do
    local out="$OUT_DIR/$(printf '%02d' $i)-${aud}.md"
    AUDIENCE="$aud" ANGLE="use-case" OUT="$out" generate_post
    i=$((i+1))
  done
}

publish_flow() {
  [[ -z "$POST" || -z "$SLUG" ]] && usage
  if [[ ! -f "$POST" ]]; then
    echo "PUBLISH FAILED: post file not found: $POST" >&2
    return 2
  fi
  if [[ ! -d "$BLOG_REPO" ]]; then
    echo "PUBLISH FAILED: JEVVELLABSBLOG_REPO not found at $BLOG_REPO" >&2
    echo "  Clone it: git clone <jevvellabsblog remote> $BLOG_REPO" >&2
    echo "  Or set env: export JEVVELLABSBLOG_REPO=/path/to/existing/clone" >&2
    return 3
  fi
  if [[ ! -d "$BLOG_REPO/_posts" ]]; then
    echo "PUBLISH FAILED: $BLOG_REPO has no _posts/ dir — not a Jekyll blog?" >&2
    return 4
  fi
  if [[ ! -f "$TEMPLATES_DIR/jekyll-frontmatter.md" ]]; then
    echo "PUBLISH FAILED: template missing: $TEMPLATES_DIR/jekyll-frontmatter.md" >&2
    return 5
  fi
  local date_prefix
  date_prefix="$(date +%Y-%m-%d)"
  local target="$BLOG_REPO/_posts/${date_prefix}-${SLUG}.md"
  if [[ -f "$target" ]]; then
    echo "PUBLISH FAILED: target already exists: $target" >&2
    echo "  Use a different --slug or remove the existing file first." >&2
    return 6
  fi
  local title
  title="$(head -1 "$POST" | sed 's/^# //')"
  {
    sed "s/{{DATE}}/$date_prefix/; s/{{SLUG}}/$SLUG/; s|{{TITLE}}|$title|" "$TEMPLATES_DIR/jekyll-frontmatter.md"
    echo ""
    tail -n +2 "$POST"  # skip the H1 since frontmatter already has it
  } > "$target"
  echo "✓ staged at $target" >&2
  echo "" >&2
  echo "To publish (you run these yourself — agent does NOT push):" >&2
  echo "  cd $BLOG_REPO" >&2
  echo "  git add _posts/${date_prefix}-${SLUG}.md" >&2
  echo "  git commit -m 'blog: $SLUG'" >&2
  echo "  git push" >&2
}

run_preflight() {
  python3.11 "$SCRIPT_DIR/preflight.py"
}

case "$CMD" in
  generate_post) generate_post;;
  generate_campaign) generate_campaign;;
  publish_flow) publish_flow;;
  preflight) run_preflight;;
  help|--help|-h) usage;;
  *) usage;;
esac
