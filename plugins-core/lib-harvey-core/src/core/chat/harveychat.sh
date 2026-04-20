#!/bin/bash
# HarveyChat — convenience wrapper
# Usage: harveychat.sh start|stop|status|setup

PYTHON="/usr/local/opt/python@3.11/bin/python3.11"
HARVEY_OS="$HOME/HARVEY/harvey-os"

cd "$HARVEY_OS" || exit 1
exec "$PYTHON" -m core.chat "$@"
