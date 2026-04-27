#!/bin/bash
# HarveyChat — convenience wrapper
# Usage: harveychat.sh start|stop|status|setup

PYTHON="/usr/local/opt/python@3.11/bin/python3.11"
MAKAKOO_HOME="${MAKAKOO_HOME:-${HARVEY_HOME:-$HOME/MAKAKOO}}"
export PYTHONPATH="$MAKAKOO_HOME/plugins/lib-harvey-core/src:$MAKAKOO_HOME/plugins/lib-hte/src${PYTHONPATH:+:$PYTHONPATH}"

cd "$MAKAKOO_HOME" || exit 1
exec "$PYTHON" -m core.chat "$@"
