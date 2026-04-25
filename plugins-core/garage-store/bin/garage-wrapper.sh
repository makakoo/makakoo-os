#!/usr/bin/env bash
# garage-store lifecycle wrapper.
#
# Invoked by the makakoo plugin runner (commands/lifecycle.rs). Three
# verbs:
#
#   start  — `launchctl load` the plist at ~/Library/LaunchAgents/com.makakoo.garage.plist.
#            The plist itself invokes `garage server -c <config>` with
#            KeepAlive + RunAtLoad. Bootstrap of the makakoo-s3-service
#            keypair fires off in the background as fire-and-forget — it
#            polls until the admin API is up, then runs `makakoo s3
#            bootstrap`.
#
#   stop   — `launchctl unload` the same plist.
#
#   health — probe the admin endpoint at 127.0.0.1:3903.
#
# launchd owns the process lifecycle. The plugin runner is just a thin
# load/unload driver. This is the "two restart mechanisms, NOT three"
# design qwen flagged in lope round 2.

set -euo pipefail

VERB="${1:-}"
PLIST_PATH="$HOME/Library/LaunchAgents/com.makakoo.garage.plist"

case "$VERB" in
    start)
        if [[ ! -f "$PLIST_PATH" ]]; then
            echo "garage-wrapper: plist missing at $PLIST_PATH" >&2
            echo "garage-wrapper: re-run \`makakoo plugin install --core garage-store\`" >&2
            exit 1
        fi
        # `launchctl load -w` enables the agent (clears Disabled flag);
        # idempotent against an already-loaded job (returns non-zero with
        # "service already loaded" — we silence that single case).
        if ! launchctl load -w "$PLIST_PATH" 2>/tmp/garage-load.err; then
            if grep -qE "already loaded|Service is enabled and loaded" /tmp/garage-load.err 2>/dev/null; then
                echo "garage-wrapper: already loaded"
            else
                cat /tmp/garage-load.err >&2
                exit 1
            fi
        fi
        echo "garage-wrapper: launchd loaded"
        # Fire-and-forget bootstrap. Polls until admin API answers, then
        # invokes `makakoo s3 bootstrap`. Never blocks the start path.
        (
            for _ in $(seq 1 60); do
                if curl -fsS -m 1 -o /dev/null http://127.0.0.1:3903/health 2>/dev/null; then
                    break
                fi
                sleep 0.5
            done
            if command -v makakoo >/dev/null 2>&1; then
                makakoo s3 bootstrap 2>&1 || \
                    echo "garage-wrapper: bootstrap exited non-zero (continuing)" >&2
            fi
        ) >/dev/null 2>&1 &
        ;;
    stop)
        if [[ ! -f "$PLIST_PATH" ]]; then
            echo "garage-wrapper: plist missing — nothing to unload"
            exit 0
        fi
        if launchctl unload -w "$PLIST_PATH" 2>/tmp/garage-unload.err; then
            echo "garage-wrapper: launchd unloaded"
        else
            if grep -qE "Could not find specified service|not currently loaded" /tmp/garage-unload.err 2>/dev/null; then
                echo "garage-wrapper: not loaded"
            else
                cat /tmp/garage-unload.err >&2
                exit 1
            fi
        fi
        ;;
    health)
        if curl -fsS -m 5 -o /dev/null http://127.0.0.1:3903/health; then
            exit 0
        fi
        exit 1
        ;;
    *)
        echo "garage-wrapper: usage: $0 {start|stop|health}" >&2
        exit 2
        ;;
esac
