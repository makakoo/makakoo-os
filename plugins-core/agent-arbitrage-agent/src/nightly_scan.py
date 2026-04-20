#!/usr/bin/env python3
"""
Polymarket Nightly Arbitrage Scanner

Runs nightly via cron to:
1. Scan for arbitrage opportunities
2. Log results to Brain
3. Send summary to logs

Cron: 0 2 * * * (2 AM daily)
"""

import sys
import os
import json
import subprocess
from datetime import datetime
from pathlib import Path

HARVEY_ROOT = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_ROOT / "data" / "arbitrage-agent"
LOG_DIR = HARVEY_ROOT / "data" / "logs"
BRAIN_BRIDGE = HARVEY_ROOT / "data" / "Brain" / "logseq_bridge.py"

sys.path.insert(
    0,
    str(HARVEY_ROOT / "harvey-os" / "skills" / "blockchain" / "polymarket" / "scripts"),
)


def log_to_file(msg: str, log_file: Path):
    """Append timestamped message to log file."""
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    log_file.parent.mkdir(parents=True, exist_ok=True)
    if log_file.exists():
        log_file.write_text(log_file.read_text() + f"[{ts}] {msg}\n")
    else:
        log_file.write_text(f"[{ts}] {msg}\n")


def run_polymarket_query(query: str) -> dict:
    """Run a polymarket query via the CLI helper."""
    result = subprocess.run(
        [
            sys.executable,
            str(
                HARVEY_ROOT
                / "harvey-os"
                / "skills"
                / "blockchain"
                / "polymarket"
                / "scripts"
                / "polymarket.py"
            ),
        ]
        + query.split(),
        capture_output=True,
        text=True,
        cwd=str(HARVEY_ROOT),
    )
    if result.returncode != 0:
        return {"error": result.stderr}
    return {"output": result.stdout}


def check_opportunities() -> dict:
    """Check for existing arbitrage opportunities."""
    opp_file = DATA_DIR / "negrisk_opportunities.json"
    if opp_file.exists():
        try:
            with open(opp_file) as f:
                return json.load(f)
        except Exception as e:
            return {"error": str(e)}
    return {"opportunities": []}


def log_to_brain(message: str):
    """Log to today's Logseq Brain journal."""
    try:
        today = datetime.now().strftime("%Y_%m_%d")
        journal = HARVEY_ROOT / "data" / "Brain" / "journals" / f"{today}.md"
        if journal.exists():
            content = journal.read_text()
            if not content.endswith("\n"):
                content += "\n"
        else:
            content = ""
        ts = datetime.now().strftime("%H:%M")
        entry = f"- [{ts}] POLYMARKET SCAN: {message}\n"
        journal.write_text(content + entry)
    except Exception as e:
        print(f"Failed to log to brain: {e}")


def main():
    print("=" * 60)
    print("POLYMARKET NIGHTLY ARBITRAGE SCAN")
    print(f"Time: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 60)

    log_file = LOG_DIR / "nightly-polymarket-scan.log"
    log_to_file("Starting nightly polymarket scan", log_file)

    print("\n[1/3] Scanning trending markets...")
    trending = run_polymarket_query("trending --limit 5")
    if "error" not in trending:
        print(f"  Found trending markets")
        log_to_file(f"Trending scan completed", log_file)
    else:
        print(f"  Error: {trending['error']}")

    print("\n[2/3] Checking arbitrage opportunities...")
    opps = check_opportunities()
    if "error" not in opps:
        opp_count = (
            len(opps.get("opportunities", opps)) if isinstance(opps, dict) else 0
        )
        print(f"  {opp_count} opportunities found")
        log_to_file(f"Opportunities check: {opp_count} found", log_file)
    else:
        print(f"  Error: {opps['error']}")

    print("\n[3/3] Logging to Brain...")
    log_to_brain(
        f"Nightly scan complete. Trending markets checked, {len(opps.get('opportunities', [])) if isinstance(opps, dict) else 0} opportunities."
    )
    print("  Done")

    log_to_file("Scan completed successfully", log_file)
    print("\n" + "=" * 60)
    print("SCAN COMPLETE")
    print("=" * 60)

    return 0


if __name__ == "__main__":
    sys.exit(main())
