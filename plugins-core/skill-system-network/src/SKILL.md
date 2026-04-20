# Network Diagnostics — "Is the internet working?"

## Overview

Interactive network diagnostic wizard that guides users through connectivity troubleshooting without technical jargon.

## Features

- **Internet check**: Verify basic connectivity (ping to 8.8.8.8)
- **Website check**: Test HTTP reachability to any URL
- **DNS resolution**: Verify domain name resolution
- **Port scanning**: Check if services are listening (localhost only for security)
- **VPN/Proxy status**: Display proxy environment configuration

## Requirements

- Python 3.8+
- Standard library only (subprocess, socket, urllib)
- No external dependencies

## Data Volume

Results are logged to: `$HARVEY_HOME/data/network-diagnostics/`
- Session logs: `checks_YYYY_MM_DD.json`

## Execution

```bash
# Run wizard
python3 -m skills.system.network.network_wizard

# Or direct invocation
cd $HARVEY_HOME/harvey-os
python3 -c "from skills.system.network.network_wizard import network_wizard; network_wizard()"
```

## Integration

Can be called programmatically:

```python
from skills.system.network.network_wizard import check_internet, check_dns, check_http

result = check_internet()  # {'connected': True/False, 'target': '8.8.8.8'}
result = check_dns('google.com')  # {'resolved': True/False, 'hostname': '...', 'ip': '...'}
result = check_http('https://google.com')  # {'reachable': True/False, 'url': '...'}
```

## Example Output

```
╔═══════════════════════════════════════╗
║  Network Diagnostics                  ║
║  Is the internet working?              ║
╚═══════════════════════════════════════╝

Step 1/1

? Choose a network check
  ► My internet connection
    A website
    A domain name
    A service port
    VPN/Proxy status

[success] Internet is reachable (pinged 8.8.8.8)
```

## Troubleshooting

**No module found**: Ensure `$HARVEY_HOME/harvey-os/` is in Python path:
```bash
PYTHONPATH=$HARVEY_HOME/harvey-os:$PYTHONPATH python3 -m skills.system.network.network_wizard
```

**Permission denied on localhost port scan**: Port scanning is intentionally restricted to localhost for security.
