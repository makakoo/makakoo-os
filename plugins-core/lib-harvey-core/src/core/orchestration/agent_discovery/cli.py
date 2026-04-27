#!/usr/bin/env python3
"""
Agent Discovery CLI
"""

import argparse
import json
import sys
import requests

BASE_URL = "http://localhost:18081"


def list_agents(capability: str = None):
    """List all registered agents."""
    params = {"capability": capability} if capability else {}
    resp = requests.get(f"{BASE_URL}/agents", params=params, timeout=5)
    resp.raise_for_status()
    data = resp.json()

    print(f"\n{'='*70}")
    print(f" Registered Agents ({data['count']})")
    print(f"{'='*70}")

    for agent in data.get("agents", []):
        print(f"\n  [{agent['agent_id']}] {agent['name']}")
        print(f"    capabilities: {', '.join(agent.get('capabilities', [])) or 'none'}")
        print(f"    skills: {', '.join(agent.get('skills', [])) or 'none'}")
        print(f"    endpoint: {agent.get('endpoint') or 'N/A'}")
        print(f"    lease expires: {agent.get('lease_expires_at', 'N/A')}")

    print(f"\n{'='*70}\n")


def show_status():
    """Show health statistics."""
    resp = requests.get(f"{BASE_URL}/agents/health", timeout=5)
    resp.raise_for_status()
    stats = resp.json()

    print(f"\n{'='*70}")
    print(f" Agent Discovery Health")
    print(f"{'='*70}")
    print(f"  Healthy: {stats.get('healthy', 0)}")
    print(f"  Stale:  {stats.get('stale', 0)}")
    print(f"  Total:  {stats.get('total', 0)}")
    print(f"{'='*70}\n")


def main():
    parser = argparse.ArgumentParser(prog="harvey discovery", description="Harvey Agent Discovery CLI")
    sub = parser.add_subparsers(dest="cmd")

    list_cmd = sub.add_parser("list", help="List registered agents")
    list_cmd.add_argument("--capability", help="Filter by capability")

    sub.add_parser("status", help="Show health statistics")

    args = parser.parse_args()

    if args.cmd == "list":
        list_agents(getattr(args, "capability", None))
    elif args.cmd == "status":
        show_status()
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
