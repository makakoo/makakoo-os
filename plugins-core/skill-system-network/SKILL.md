# Network Diagnostics — "Is the internet working?"

## Overview

Interactive network diagnostic wizard that guides users through connectivity troubleshooting without technical jargon. Shipped as a makakoo plugin; entrypoint resolves to the Python wizard at `$MAKAKOO_HOME/harvey-os/skills/system/network/network_wizard.py`.

## Features

- **Internet check**: Verify basic connectivity (ping to 8.8.8.8)
- **Website check**: Test HTTP reachability to any URL
- **DNS resolution**: Verify domain name resolution
- **Port scanning**: Check if services are listening (localhost only for security)
- **VPN/Proxy status**: Display proxy environment configuration

## Requirements

- Python 3.8+
- Standard library only (subprocess, socket, urllib)
- `ping` binary on PATH
- HTE framework at `$MAKAKOO_HOME/harvey-os/core/terminal/` (shipped with Harvey)

## Data volume

Results are logged to: `$MAKAKOO_HOME/state/skill-system-network/`
- Session logs: `checks_YYYY_MM_DD.json`

## Execution

```bash
# Via makakoo CLI (post-install)
makakoo skill run skill-system-network

# Direct invocation
python3 -u $MAKAKOO_HOME/harvey-os/skills/system/network/network_wizard.py
```

## Capabilities declared

- `net/http` — HTTP reachability probes
- `exec/binary:ping` — ping 8.8.8.8 for basic connectivity

## Caveat (2026-04-16)

The wizard imports `from core.terminal import Wizard, ...` which resolves against `$MAKAKOO_HOME/harvey-os/core/terminal/`. When the HTE framework is eventually vendored into `makakoo-os` as a pure-Rust TUI kernel primitive or as a bundled Python package, this entrypoint will be updated. Until then it runs against Sebastian's live Python tree at `/Users/sebastian/MAKAKOO/harvey-os/`.
