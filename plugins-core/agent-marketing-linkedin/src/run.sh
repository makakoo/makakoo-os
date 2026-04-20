#!/usr/bin/env bash
# marketing-linkedin — LinkedIn post generator using the 10-theme wheel.
#
# Usage:
#   run.sh generate_post --brief BRIEF.md --theme launch-day --product lope --out OUT.md
#   run.sh generate_wheel --brief BRIEF.md --product lope --out-dir DIR/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"

OPENAI_BASE_URL="${OPENAI_BASE_URL:-http://localhost:18080/v1}"
OPENAI_API_KEY="${OPENAI_API_KEY:-$(grep -E "^LLM_API_KEY=" "$HARVEY_HOME/.env" 2>/dev/null | cut -d= -f2- || echo sk-test-123)}"
MODEL="${MARKETING_LINKEDIN_MODEL:-minimax:MiniMax-M2.7}"

SKILL_MD="$SCRIPT_DIR/SKILL.md"
THEMES_MD="$SCRIPT_DIR/templates/themes.md"
EXAMPLES_DIR="$SCRIPT_DIR/examples"

# Canonical theme order — matches wheel in SKILL.md
THEMES=(
  "launch-day"
  "single-insight"
  "killer-feature"
  "personality"
  "use-case-expansion"
  "lore"
  "origin"
  "dogfood-receipt"
  "install-cta"
  "community-call"
)

usage() {
  cat <<EOF
Usage:
  GENERATE:
    $0 generate_post --brief BRIEF.md --theme THEME --product NAME --out OUT.md
    $0 generate_wheel --brief BRIEF.md --product NAME --out-dir DIR/

  POST (real LinkedIn UGC Posts API):
    $0 preflight                     # check LINKEDIN_ACCESS_TOKEN + scope
    $0 post_text --file POST.md [--dry-run]

Themes: ${THEMES[*]}

Required env for posting (in ~/MAKAKOO/.env):
  LINKEDIN_ACCESS_TOKEN  (OAuth 2.0 user token with w_member_social scope,
                          acquired via authorization code flow from a
                          LinkedIn Developer App — tokens expire in 60 days)
EOF
  exit 1
}

CMD="${1:-}"; shift || usage

BRIEF=""; THEME="launch-day"; PRODUCT=""; OUT=""; OUT_DIR=""; FILE=""; DRY_RUN=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --brief) BRIEF="$2"; shift 2;;
    --theme) THEME="$2"; shift 2;;
    --product) PRODUCT="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --out-dir) OUT_DIR="$2"; shift 2;;
    --file) FILE="$2"; shift 2;;
    --dry-run) DRY_RUN="1"; shift;;
    *) echo "unknown flag: $1"; usage;;
  esac
done

call_llm() {
  local system_prompt="$1"
  local user_prompt="$2"
  python3 - "$system_prompt" "$user_prompt" <<'PY'
import json, os, sys, urllib.request
system_prompt, user_prompt = sys.argv[1], sys.argv[2]
base = os.environ["OPENAI_BASE_URL"].rstrip("/")
key = os.environ["OPENAI_API_KEY"]
model = os.environ["MODEL"]
payload = {
    "model": model,
    "messages": [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_prompt},
    ],
    "temperature": 0.75,
    "max_tokens": 800,
}
req = urllib.request.Request(
    f"{base}/chat/completions",
    data=json.dumps(payload).encode(),
    headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=180) as resp:
    body = json.loads(resp.read())
print(body["choices"][0]["message"]["content"])
PY
}

build_system_prompt() {
  local theme="$1"
  cat "$SKILL_MD"
  echo ""
  cat "$THEMES_MD"
  echo ""
  echo "## Reference example for theme: $theme"
  # Try to find a matching example
  local ref=""
  case "$theme" in
    launch-day) ref="01-launch-day.md";;
    single-insight) ref="02-single-model-blindspot.md";;
    killer-feature) ref="03-mistral-vibe.md";;
    personality) ref="04-caveman-mode.md";;
    use-case-expansion) ref="05-not-just-code.md";;
    lore) ref="06-lion-face-easter-egg.md";;
    origin) ref="07-origin.md";;
    dogfood-receipt) ref="08-dogfood-receipt.md";;
    install-cta) ref="09-install-copy-paste.md";;
    community-call) ref="10-call-for-validators.md";;
  esac
  if [[ -n "$ref" && -f "$EXAMPLES_DIR/$ref" ]]; then
    cat "$EXAMPLES_DIR/$ref"
  fi
}

generate_post() {
  [[ -z "$BRIEF" || -z "$PRODUCT" || -z "$OUT" ]] && usage
  [[ ! -f "$BRIEF" ]] && { echo "brief not found: $BRIEF"; exit 2; }
  local brief_content system_prompt user_prompt
  brief_content="$(cat "$BRIEF")"
  system_prompt="$(build_system_prompt "$THEME")"
  user_prompt="Campaign brief:
$brief_content

Target theme: $THEME
Product: $PRODUCT

Write a LinkedIn post for this theme. 150-300 words. First line is the hook — blank line after, then body. End with CTA + 4-5 hashtags. Match the voice of the reference example exactly. Output pure post text — no preamble, no JSON, no explanation."
  echo "Generating LinkedIn post: $PRODUCT / $THEME → $OUT" >&2
  OPENAI_BASE_URL="$OPENAI_BASE_URL" OPENAI_API_KEY="$OPENAI_API_KEY" MODEL="$MODEL" \
    call_llm "$system_prompt" "$user_prompt" > "$OUT"
  echo "✓ wrote $OUT ($(wc -w < "$OUT") words)" >&2
}

generate_wheel() {
  [[ -z "$BRIEF" || -z "$PRODUCT" || -z "$OUT_DIR" ]] && usage
  mkdir -p "$OUT_DIR"
  local i=1
  for t in "${THEMES[@]}"; do
    local out="$OUT_DIR/$(printf '%02d' $i)-${t}.md"
    THEME="$t" OUT="$out" generate_post
    i=$((i+1))
  done
  echo "✓ full wheel written to $OUT_DIR (10 posts)" >&2
}

run_preflight() {
  python3.11 "$SCRIPT_DIR/post_linkedin.py" preflight
}

run_post_text() {
  [[ -z "$FILE" ]] && usage
  local args=(post --file "$FILE")
  [[ -n "$DRY_RUN" ]] && args+=(--dry-run)
  python3.11 "$SCRIPT_DIR/post_linkedin.py" "${args[@]}"
}

case "$CMD" in
  generate_post) generate_post;;
  generate_wheel) generate_wheel;;
  preflight) run_preflight;;
  post_text) run_post_text;;
  help|--help|-h) usage;;
  *) usage;;
esac
