#!/bin/bash
# Cron wrapper for Auto-Improver nightly skill review
# Runs skill improvement cycle with real LLM calls (no money involved)

HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"
PYTHON="/usr/local/opt/python@3.11/bin/python3.11"
SCRIPT="$HARVEY_HOME/plugins-core/meta/autoimprover/run_improvements.py"
LOG="$HARVEY_HOME/tmp/autoimprover.log"

exec "$PYTHON" "$SCRIPT" >> "$LOG" 2>&1
