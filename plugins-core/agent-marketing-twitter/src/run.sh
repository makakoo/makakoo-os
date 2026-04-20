#!/usr/bin/env bash
# marketing-twitter — Twitter/X launch thread generator.
#
# Usage:
#   run.sh generate_thread --brief BRIEF.md --product lope --tweet-count 12 --style fact --out OUT.md
#   run.sh count_chars --file THREAD.md

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"

OPENAI_BASE_URL="${OPENAI_BASE_URL:-http://localhost:18080/v1}"
OPENAI_API_KEY="${OPENAI_API_KEY:-$(grep -E "^LLM_API_KEY=" "$HARVEY_HOME/.env" 2>/dev/null | cut -d= -f2- || echo sk-test-123)}"
MODEL="${MARKETING_TWITTER_MODEL:-minimax:MiniMax-M2.7}"

SKILL_MD="$SCRIPT_DIR/SKILL.md"
EXAMPLES_DIR="$SCRIPT_DIR/examples"

usage() {
  cat <<EOF
Usage:
  GENERATE:
    $0 generate_thread --brief BRIEF.md --product NAME [--tweet-count 12] [--style fact|question|provocation] --out OUT.md
    $0 count_chars --file THREAD.md

  POST (real X API):
    $0 preflight                                  # check X API creds + tweepy
    $0 parse --file THREAD.md                      # parse + print tweets, no network
    $0 post_thread --file THREAD.md [--images DIR] [--dry-run]

Required env for posting (in ~/MAKAKOO/.env):
  X_API_KEY, X_API_SECRET, X_ACCESS_TOKEN, X_ACCESS_SECRET
  (OAuth 1.0a user context from developer.x.com — app needs Read+Write)
EOF
  exit 1
}

CMD="${1:-}"; shift || usage

BRIEF=""; PRODUCT=""; TWEET_COUNT="12"; STYLE="fact"; OUT=""; FILE=""; IMAGES=""; DRY_RUN=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --brief) BRIEF="$2"; shift 2;;
    --product) PRODUCT="$2"; shift 2;;
    --tweet-count) TWEET_COUNT="$2"; shift 2;;
    --style) STYLE="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --file) FILE="$2"; shift 2;;
    --images) IMAGES="$2"; shift 2;;
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
    "temperature": 0.7,
    "max_tokens": 2500,
}
req = urllib.request.Request(
    f"{base}/chat/completions",
    data=json.dumps(payload).encode(),
    headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=240) as resp:
    body = json.loads(resp.read())
print(body["choices"][0]["message"]["content"])
PY
}

generate_thread() {
  [[ -z "$BRIEF" || -z "$PRODUCT" || -z "$OUT" ]] && usage
  [[ ! -f "$BRIEF" ]] && { echo "brief not found: $BRIEF"; exit 2; }
  local brief_content system_prompt user_prompt example
  brief_content="$(cat "$BRIEF")"
  example=""
  [[ -f "$EXAMPLES_DIR/lope-twitter-thread.md" ]] && example="$(cat "$EXAMPLES_DIR/lope-twitter-thread.md")"
  system_prompt="$(cat "$SKILL_MD")

## Reference example (frozen — match this quality bar)
$example"
  user_prompt="Campaign brief:
$brief_content

Product: $PRODUCT
Tweet count: $TWEET_COUNT
Style: $STYLE

Generate a launch Twitter thread matching the structure in the system prompt. Each tweet must be strictly under 280 characters. Use the numbered '**Tweet N (role)**' format. Separate tweets with a blank line then '---' then blank line. Output pure markdown, no preamble, no JSON, no explanation. The hook (tweet 1) must stand alone if retweeted without the rest."
  echo "Generating Twitter thread: $PRODUCT / $STYLE / $TWEET_COUNT tweets → $OUT" >&2
  local tmp="$(mktemp)"
  OPENAI_BASE_URL="$OPENAI_BASE_URL" OPENAI_API_KEY="$OPENAI_API_KEY" MODEL="$MODEL" \
    call_llm "$system_prompt" "$user_prompt" > "$tmp"
  # Strict gate: refuse to write output if any tweet overruns 280 chars.
  # This kills the hallucination class where the agent generates plausible-looking
  # but unpostable content and downstream agents invent excuses ("API can't post long tweets").
  if ! count_chars_impl "$tmp" >&2; then
    echo "REJECTED: $(basename "$OUT") has over-limit tweets — NOT written." >&2
    echo "  Draft saved for inspection: $tmp" >&2
    echo "  Regenerate with a shorter hook or trim content before use." >&2
    return 1
  fi
  mv "$tmp" "$OUT"
  echo "✓ wrote $OUT" >&2
}

count_chars_impl() {
  local f="$1"
  [[ ! -f "$f" ]] && { echo "file not found: $f"; exit 2; }
  python3 - "$f" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
# Split on "**Tweet N" headers
parts = re.split(r'\n?\*\*Tweet (\d+)[^*]*\*\*\n+', text)
# parts alternates: [prefix, n1, body1, n2, body2, ...]
over = 0
checked = 0
for i in range(1, len(parts), 2):
    n = parts[i]
    body = parts[i+1] if i+1 < len(parts) else ""
    # Strip trailing --- separator and whitespace
    body = re.sub(r'\n+---\n*$', '', body).strip()
    # Remove leading/trailing whitespace but preserve internal structure
    body = body.strip()
    length = len(body)
    status = "✓" if length <= 280 else "✗ OVER"
    if length > 280:
        over += 1
    print(f"  tweet {n}: {length}/280 {status}")
    checked += 1
print(f"\nTotal: {checked} tweets, {over} over the limit")
if over:
    sys.exit(1)
PY
}

count_chars() {
  [[ -z "$FILE" ]] && usage
  count_chars_impl "$FILE"
}

run_preflight() {
  python3.11 "$SCRIPT_DIR/post_thread.py" preflight
}

run_parse() {
  [[ -z "$FILE" ]] && usage
  python3.11 "$SCRIPT_DIR/post_thread.py" parse --file "$FILE"
}

run_post_thread() {
  [[ -z "$FILE" ]] && usage
  local args=(post --file "$FILE")
  [[ -n "$IMAGES" ]] && args+=(--images "$IMAGES")
  [[ -n "$DRY_RUN" ]] && args+=(--dry-run)
  python3.11 "$SCRIPT_DIR/post_thread.py" "${args[@]}"
}

case "$CMD" in
  generate_thread) generate_thread;;
  count_chars) count_chars;;
  preflight) run_preflight;;
  parse) run_parse;;
  post_thread) run_post_thread;;
  help|--help|-h) usage;;
  *) usage;;
esac
