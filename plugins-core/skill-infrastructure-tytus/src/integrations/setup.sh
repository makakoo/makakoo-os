#!/bin/bash
# Tytus — Quick setup for any CLI tool
# Source this file: source setup.sh
#
# Prerequisites:
#   1. tytus CLI installed (cargo build --release -p atomek-cli)
#   2. Logged in: tytus login
#   3. Tunnel running: sudo tytus connect (separate terminal)

if ! command -v tytus &> /dev/null; then
    echo "Error: tytus not installed. Build from:"
    echo "  cd /path/to/tytus-cli && cargo build --release -p atomek-cli"
    echo "  sudo cp target/release/tytus /usr/local/bin/"
    return 1
fi

if ! tytus status --json 2>/dev/null | grep -q '"logged_in"'; then
    echo "Error: not logged in. Run: tytus login"
    return 1
fi

# Export Tytus vars
eval $(tytus env --export 2>/dev/null)

if [ -z "$TYTUS_AI_GATEWAY" ]; then
    echo "Error: no active pod. Run: sudo tytus connect (in separate terminal)"
    return 1
fi

# Also set OpenAI-compatible vars for tools that expect them
export OPENAI_API_KEY=$TYTUS_API_KEY
export OPENAI_BASE_URL=${TYTUS_AI_GATEWAY}/v1

echo "Tytus ready:"
echo "  Gateway:  $TYTUS_AI_GATEWAY"
echo "  Agent:    ${TYTUS_AGENT_TYPE:-unknown}"
echo "  Pod:      $TYTUS_POD_ID"
echo "  OPENAI_BASE_URL=$OPENAI_BASE_URL"
