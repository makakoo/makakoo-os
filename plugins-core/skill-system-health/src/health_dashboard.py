"""System Health Dashboard — Monitor Harvey services and machine resources."""

import os
import sys
import json
import subprocess
from pathlib import Path
from typing import Dict, Any
import psutil  # May not be available, handled gracefully

# Add parent to path for HTE import
sys.path.insert(0, str(Path(__file__).parent.parent.parent.parent.parent))

from core.terminal import Header, Table, Column, Panel, StatusBadge, Alert
from core.terminal.animation import ProgressAnimation


class HealthMonitor:
    """Monitor Harvey services and system health."""

    def __init__(self):
        self.SERVICES = {
            "switchAILocal": {
                "url": "http://localhost:18080/health",
                "type": "http",
            },
            "Qdrant": {
                "url": "http://localhost:6333/health",
                "type": "http",
            },
            "PostgreSQL": {
                "host": "localhost",
                "port": 5434,
                "type": "postgres",
            },
            "Logseq (optional)": {
                "url": "http://127.0.0.1:12315/version",
                "type": "http",
            },
        }

    def check_http_service(self, url: str) -> bool:
        """Check if HTTP service is responding."""
        try:
            import urllib.request
            import urllib.error

            response = urllib.request.urlopen(url, timeout=2)
            return response.status in (200, 204)
        except Exception:
            return False

    def check_postgres(self, host: str, port: int) -> bool:
        """Check PostgreSQL connectivity."""
        try:
            import socket

            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(2)
            result = sock.connect_ex((host, port))
            sock.close()
            return result == 0
        except Exception:
            return False

    def get_service_status(self) -> Dict[str, Dict[str, Any]]:
        """Check all services."""
        status = {}

        for service_name, config in self.SERVICES.items():
            if config["type"] == "http":
                is_healthy = self.check_http_service(config["url"])
            elif config["type"] == "postgres":
                is_healthy = self.check_postgres(config["host"], config["port"])
            else:
                is_healthy = False

            status[service_name] = {
                "healthy": is_healthy,
                "status": "running" if is_healthy else "stopped",
            }

        return status

    def get_system_resources(self) -> Dict[str, Any]:
        """Get CPU, memory, and disk usage."""
        try:
            import psutil

            cpu_percent = psutil.cpu_percent(interval=0.5)
            memory = psutil.virtual_memory()
            disk = psutil.disk_usage("/")

            return {
                "cpu_percent": cpu_percent,
                "memory_percent": memory.percent,
                "memory_available": self._format_bytes(memory.available),
                "disk_percent": disk.percent,
                "disk_free": self._format_bytes(disk.free),
            }
        except ImportError:
            # psutil not available
            return {
                "cpu_percent": "N/A",
                "memory_percent": "N/A",
                "memory_available": "N/A",
                "disk_percent": "N/A",
                "disk_free": "N/A",
            }

    def _format_bytes(self, bytes_val: int) -> str:
        """Format bytes to human readable."""
        for unit in ("B", "KB", "MB", "GB", "TB"):
            if bytes_val < 1024:
                return f"{bytes_val:.1f}{unit}"
            bytes_val /= 1024
        return f"{bytes_val:.1f}TB"

    def get_status_icon(self, is_healthy: bool) -> str:
        """Get status icon."""
        return "✅" if is_healthy else "❌"

    def display_dashboard(self) -> None:
        """Display health dashboard."""
        print("\n" + Header("System Health", "Harvey Services & Resources").render() + "\n")

        # Services status
        print(f"Checking services...")
        progress = ProgressAnimation(3, "Checking")

        service_status = self.get_service_status()
        progress.step("Done", 2)
        progress.done()

        print("\n=== Harvey Services ===\n")

        for service_name, status in service_status.items():
            icon = self.get_status_icon(status["healthy"])
            status_text = f"{icon} {service_name}"
            if status["healthy"]:
                print(f"{status_text.ljust(30)} {StatusBadge('success').render()}")
            else:
                print(f"{status_text.ljust(30)} {StatusBadge('error').render()}")

        # System resources
        print("\n=== System Resources ===\n")
        resources = self.get_system_resources()

        if resources["cpu_percent"] != "N/A":
            cpu_bar = self._progress_bar(float(resources["cpu_percent"]), 100)
            print(f"CPU:    {cpu_bar} {resources['cpu_percent']:.1f}%")

            mem_bar = self._progress_bar(float(resources["memory_percent"]), 100)
            print(f"Memory: {mem_bar} {resources['memory_percent']:.1f}%")

            disk_bar = self._progress_bar(float(resources["disk_percent"]), 100)
            print(f"Disk:   {disk_bar} {resources['disk_percent']:.1f}%")
        else:
            print("(System monitoring unavailable — install psutil: pip install psutil)")

        print()

    def _progress_bar(self, current: float, total: float, width: int = 20) -> str:
        """Simple progress bar."""
        percent = current / total
        filled = int(width * percent)
        return "[" + "█" * filled + "░" * (width - filled) + "]"

    def interactive_dashboard(self) -> None:
        """Run interactive dashboard with refresh capability."""
        while True:
            self.display_dashboard()

            print("Options:")
            print("  r - Refresh | q - Quit")

            choice = input("\nChoice (r/q): ").strip().lower()

            if choice == "q" or not choice:
                break
            elif choice == "r":
                print("\n\033[2J\033[H")  # Clear screen


def health_dashboard_main():
    """Run system health dashboard."""
    monitor = HealthMonitor()
    monitor.interactive_dashboard()
    print("\nDashboard closed.\n")


if __name__ == "__main__":
    health_dashboard_main()
