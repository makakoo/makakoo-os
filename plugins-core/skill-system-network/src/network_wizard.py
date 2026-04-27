"""Network Wizard — "Is the internet working?" guided diagnostic tool."""

import subprocess
import socket
import sys
from typing import Dict, Any
from pathlib import Path

# Add parent to path for HTE import
sys.path.insert(0, str(Path(__file__).parent.parent.parent.parent.parent))

from core.terminal import Wizard, WizardStep, Panel, StatusBadge, Header
from core.terminal.animation import ProgressAnimation


def check_internet() -> Dict[str, Any]:
    """Check basic internet connectivity via ping."""
    try:
        # Ping Google DNS
        result = subprocess.run(
            ["ping", "-c", "1", "-W", "2", "8.8.8.8"],
            capture_output=True,
            timeout=5,
        )
        return {"connected": result.returncode == 0, "target": "8.8.8.8"}
    except Exception as e:
        return {"connected": False, "error": str(e)}


def check_dns(hostname: str) -> Dict[str, Any]:
    """Check DNS resolution."""
    try:
        ip = socket.gethostbyname(hostname)
        return {"resolved": True, "hostname": hostname, "ip": ip}
    except socket.gaierror as e:
        return {"resolved": False, "hostname": hostname, "error": str(e)}


def check_http(url: str) -> Dict[str, Any]:
    """Check HTTP connectivity."""
    try:
        import urllib.request
        import urllib.error

        response = urllib.request.urlopen(url, timeout=5)
        return {"reachable": response.status == 200, "url": url, "status": response.status}
    except urllib.error.URLError as e:
        return {"reachable": False, "url": url, "error": str(e)}
    except Exception as e:
        return {"reachable": False, "url": url, "error": str(e)}


def check_port(host: str, port: int) -> Dict[str, Any]:
    """Check if a port is open (localhost only)."""
    if not host.startswith("localhost") and not host.startswith("127."):
        return {"error": "Port scanning only allowed on localhost for security"}

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(2)
    result = sock.connect_ex((host, port))
    sock.close()

    return {"open": result == 0, "host": host, "port": port}


def check_vpn() -> Dict[str, Any]:
    """Check VPN/proxy status."""
    import os

    proxy_vars = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
    active_proxies = [var for var in proxy_vars if os.environ.get(var)]

    return {"proxy_configured": len(active_proxies) > 0, "proxies": active_proxies}


def network_wizard():
    """Run interactive network diagnostic wizard."""
    print("\n" + Header("Network Diagnostics", "Is the internet working?").render() + "\n")

    steps = [
        WizardStep(
            id="check_type",
            title="What would you like to check?",
            prompt="Choose a network check",
            help_text="Select which network component to diagnose.",
            input_type="select",
            choices=["My internet connection", "A website", "A domain name", "A service port", "VPN/Proxy status"],
            default="My internet connection",
        ),
    ]

    wizard = Wizard("Network Diagnostics", steps)
    results = wizard.run()

    if not results:
        return

    check_type = results.get("check_type", "")

    # Run appropriate check
    print("\n")
    progress = ProgressAnimation(4, "Running diagnostics")

    if check_type == "My internet connection":
        progress.step("Checking DNS", 1)
        result = check_internet()
        progress.step("Done", 3)
        progress.done()

        if result.get("connected"):
            status = StatusBadge("success").render()
            print(f"{status} Internet is reachable (pinged {result['target']})\n")
        else:
            status = StatusBadge("error").render()
            error = result.get("error", "Connection timeout")
            print(f"{status} Internet is unreachable: {error}\n")
            print("Troubleshooting tips:")
            print("  1. Check your WiFi/Ethernet connection")
            print("  2. Try restarting your router")
            print("  3. Check if your ISP is having issues\n")

    elif check_type == "A website":
        url = input("Website URL (e.g. google.com): ").strip()
        if url and not url.startswith("http"):
            url = f"https://{url}"

        progress.step("Resolving DNS", 1)
        dns_result = check_dns(url.split("/")[2] if url else "")
        progress.step("Checking HTTP", 2)
        http_result = check_http(url)
        progress.step("Done", 3)
        progress.done()

        if http_result.get("reachable"):
            status = StatusBadge("success").render()
            print(f"{status} Website is reachable ({url})\n")
        else:
            status = StatusBadge("error").render()
            error = http_result.get("error", "Connection timeout")
            print(f"{status} Website is unreachable: {error}\n")

    elif check_type == "A domain name":
        hostname = input("Domain/Hostname (e.g. google.com): ").strip()

        progress.step("Resolving DNS", 1)
        result = check_dns(hostname)
        progress.done()

        if result.get("resolved"):
            status = StatusBadge("success").render()
            print(f"{status} {hostname} resolves to {result['ip']}\n")
        else:
            status = StatusBadge("error").render()
            print(f"{status} {hostname} could not be resolved\n")
            print("Possible causes:")
            print("  1. Domain doesn't exist")
            print("  2. DNS server is unreachable")
            print("  3. Firewall is blocking DNS\n")

    elif check_type == "A service port":
        host = input("Host (default: localhost): ").strip() or "localhost"
        try:
            port = int(input("Port number: ").strip())
        except ValueError:
            print("Invalid port number.")
            return

        progress.step("Checking port", 1)
        result = check_port(host, port)
        progress.done()

        if result.get("error"):
            status = StatusBadge("warning").render()
            print(f"{status} {result['error']}\n")
        elif result.get("open"):
            status = StatusBadge("success").render()
            print(f"{status} Port {port} is open on {host}\n")
        else:
            status = StatusBadge("error").render()
            print(f"{status} Port {port} is closed on {host}\n")

    elif check_type == "VPN/Proxy status":
        result = check_vpn()
        progress.step("Checking", 1)
        progress.done()

        if result.get("proxy_configured"):
            status = StatusBadge("info").render()
            print(f"{status} Proxy/VPN is configured:\n")
            for proxy in result.get("proxies", []):
                print(f"  - {proxy}: {os.environ.get(proxy)}")
            print()
        else:
            status = StatusBadge("success").render()
            print(f"{status} No proxy/VPN configured (direct connection)\n")


if __name__ == "__main__":
    network_wizard()
