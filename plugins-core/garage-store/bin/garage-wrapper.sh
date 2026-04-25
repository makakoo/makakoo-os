#!/usr/bin/env bash
# garage-store lifecycle wrapper (Phase D adapter shim).
#
# Daemon lifecycle moved to the standalone `garagetytus` binary as
# of GARAGETYTUS-V0.1. This script now defers to it; the original
# launchctl + garage-cli orchestration is preserved in git history.

set -euo pipefail

ACTION="${1:-status}"

if ! command -v garagetytus >/dev/null 2>&1; then
    echo "garage-store: garagetytus not found — install at https://garagetytus.dev" >&2
    exit 1
fi

case "${ACTION}" in
    start)   exec garagetytus start ;;
    stop)    exec garagetytus stop ;;
    status|health) exec garagetytus status ;;
    *)
        echo "garage-store: unknown action: ${ACTION}" >&2
        echo "  valid: start | stop | status | health" >&2
        exit 1
        ;;
esac
