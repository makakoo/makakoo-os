#!/usr/local/opt/python@3.11/bin/python3.11
"""
Session Summary — Generates a session log entry and updates STATE.md.

Usage:
    python3 session_summary.py                        # interactive
    python3 session_summary.py --message "Did X"     # one-liner
    python3 session_summary.py --update                # just refresh STATE.md timestamp
    python3 session_summary.py --dry-run             # show what would be written
    python3 session_summary.py --last                 # show last session entry

Run at end of every Harvey session to keep STATE.md current.

This script is the M2: Project Governance deliverable — standardizing
STATE.md updates so they happen consistently without manual effort.
"""

import argparse
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

# Paths
import os as _os
HARVEY = Path(_os.environ.get("HARVEY_HOME", _os.path.expanduser("~/MAKAKOO")))
STATE_MD = HARVEY / ".planning" / "STATE.md"
LOG_MARKER = "## 📅 Session Log"
FOOTER_MARKER = "*Last updated:"

# Load current STATE.md content
def get_state_content() -> str:
    if STATE_MD.exists():
        return STATE_MD.read_text()
    return ""


def get_last_entry(log_content: str) -> str:
    """Extract the most recent session log entry for context."""
    if LOG_MARKER not in log_content:
        return ""
    log_section = log_content.split(LOG_MARKER)[1]
    if FOOTER_MARKER in log_section:
        log_section = log_section.split(FOOTER_MARKER)[0]
    # Get first few lines after the marker
    lines = log_section.strip().split("\n")
    # Skip the first "-" line and return next few entries
    return "\n".join(lines[:8])


def build_new_entry(message: str, timestamp: str) -> str:
    """Build a new session log entry."""
    # Get date portion of timestamp
    date = timestamp[:10]  # "2026-03-28"
    entry = f"- **{date}**:\n"
    for line in message.strip().split("\n"):
        entry += f"  - {line}\n"
    return entry.rstrip()


def get_git_diff_summary() -> str:
    """Get a brief summary of files changed in this session."""
    try:
        result = subprocess.run(
            ["git", "-C", str(HARVEY), "diff", "--stat", "HEAD"],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0 and result.stdout.strip():
            lines = result.stdout.strip().split("\n")
            # Return last 3 lines of diff stat
            return "\n".join(lines[-3:])
    except Exception:
        pass
    return ""


def update_state_md(message: str, dry_run: bool = False) -> bool:
    """Append a new session log entry to STATE.md and update timestamp."""
    state_content = get_state_content()
    timestamp = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
    new_entry = build_new_entry(message, timestamp)

    if not state_content:
        print("ERROR: STATE.md not found or empty", file=sys.stderr)
        return False

    # Check if today's entry already exists (avoid duplicate dates)
    today = timestamp[:10]
    if f"- **{today}**:" in state_content:
        # Append to existing today's entry - find the section
        # We insert content right after the "- **YYYY-MM-DD**:" line
        lines = state_content.split("\n")
        for i, line in enumerate(lines):
            if line.strip() == f"- **{today}**:":
                # Find where this section ends (next "- **" or footer)
                insert_idx = i + 1
                while insert_idx < len(lines):
                    next_line = lines[insert_idx].strip()
                    if next_line.startswith("- **") or next_line.startswith("*Last"):
                        break
                    insert_idx += 1
                # Insert new bullet points before the next date or footer
                indent = "  " if lines[i+1].startswith("  ") else ""
                for j, msg_line in enumerate(message.strip().split("\n")):
                    lines.insert(insert_idx + j, f"{indent}- {msg_line}")
                state_content = "\n".join(lines)
                break
    else:
        # Append new entry before the footer
        if FOOTER_MARKER in state_content:
            state_content = state_content.replace(
                FOOTER_MARKER,
                new_entry + "\n\n" + FOOTER_MARKER
            )
        else:
            state_content += "\n\n" + new_entry

    # Update timestamp
    new_footer = f"*Last updated: {timestamp} (System Time)*"
    if FOOTER_MARKER in state_content:
        state_content = state_content.split(FOOTER_MARKER)[0] + new_footer
    else:
        state_content += "\n\n" + new_footer

    if dry_run:
        print("=== DRY RUN — would write ===")
        print(state_content)
        print("=== END DRY RUN ===")
        return True

    STATE_MD.write_text(state_content)
    return True


def main():
    parser = argparse.ArgumentParser(description="Session Summary — update STATE.md")
    parser.add_argument("--message", "-m", type=str, default=None,
                        help="Session summary message (multi-line for multiple bullets)")
    parser.add_argument("--update", "-u", action="store_true",
                        help="Just refresh the timestamp, no new entry")
    parser.add_argument("--dry-run", action="store_true",
                        help="Show what would be written without writing")
    parser.add_argument("--last", action="store_true",
                        help="Show the last session log entry")

    args = parser.parse_args()

    if args.last:
        content = get_state_content()
        entry = get_last_entry(content)
        if entry:
            print("Last session entry:\n")
            print(entry)
        else:
            print("No previous session entry found.")
        return

    if args.update:
        # Just refresh timestamp
        state_content = get_state_content()
        timestamp = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
        new_footer = f"*Last updated: {timestamp} (System Time)*"
        if FOOTER_MARKER in state_content:
            state_content = state_content.split(FOOTER_MARKER)[0] + new_footer
        if args.dry_run:
            print("=== DRY RUN — would write ===")
            print(state_content)
            print("=== END DRY RUN ===")
        else:
            STATE_MD.write_text(state_content)
            print(f"Timestamp refreshed: {timestamp}")
        return

    if args.message:
        # Auto-commit the current git diff as context
        diff_summary = get_git_diff_summary()
        message = args.message
        if diff_summary:
            message = f"{args.message}\n\nFiles changed:\n{diff_summary}"
        success = update_state_md(message, dry_run=args.dry_run)
        if success:
            if args.dry_run:
                print("Dry run complete.")
            else:
                print(f"Session log updated in STATE.md")
        return

    # Interactive mode — no args
    print("Session Summary")
    print("=" * 50)
    print()
    print("Paste your session summary (Ctrl+D to finish, Ctrl+C to abort):")
    print("(Multi-line: each line becomes a bullet point)")
    lines = []
    try:
        while True:
            line = input()
            lines.append(line)
    except EOFError:
        pass

    if not lines:
        print("No input — aborting.")
        sys.exit(1)

    message = "\n".join(lines)
    success = update_state_md(message, dry_run=False)
    if success:
        print(f"\nSession log written to {STATE_MD}")


if __name__ == "__main__":
    main()
