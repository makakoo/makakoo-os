#!/usr/bin/env python3
"""
Logseq plugin for Makakoo OS.

Connects Makakoo Brain to Logseq app.
Brain already uses Logseq markdown format - just point Logseq to Brain folder.

Usage:
    makakoo skill logseq status
    makakoo skill logseq connect
    makakoo skill logseq pages
    makakoo skill logseq search <query>
"""

import os
import sys
import json
from pathlib import Path
from datetime import date

# Import real brain_bridge (the actual implementation)
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "lib-harvey-core" / "src"))
try:
    from core.memory import brain_bridge as bb
except ImportError:
    bb = None


def get_brain_path() -> Path:
    home = Path(os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", Path.home() / "MAKAKOO")))
    return home / "data" / "Brain"


def get_logseq_vault_path() -> Path:
    return Path(os.environ.get("LOGSEQ_VAULT_PATH", Path.home() / "Documents" / "Logseq"))


def status():
    """Show current status."""
    brain = get_brain_path()
    logseq = get_logseq_vault_path()
    
    print("=== Logseq Integration Status ===\n")
    print(f"Brain: {brain}")
    print(f"  Exists: {brain.exists()}")
    
    if brain.exists():
        pages_dir = brain / "pages"
        journals_dir = brain / "journals"
        pages = len(list(pages_dir.rglob("*.md"))) if pages_dir.exists() else 0
        journals = len(list(journals_dir.glob("*.md"))) if journals_dir.exists() else 0
        print(f"  Pages: {pages}")
        print(f"  Journals: {journals}")
        
        # Try brain_bridge for more info
        if bb:
            try:
                all_pages = bb.get_all_pages(limit=5)
                print(f"\n  Recent pages: {len(all_pages) if all_pages else 0}")
            except:
                pass
    
    print(f"\nLogseq Vault: {logseq}")
    print(f"  Exists: {logseq.exists()}")
    
    if logseq.exists():
        logseq_notes = len(list(logseq.rglob("*.md")))
        print(f"  Notes: {logseq_notes}")


def connect():
    """Print setup instructions."""
    brain = get_brain_path()
    
    print("""
=== Connect Logseq App to Brain ===

1. Download Logseq from https://logseq.com
2. Open Logseq app
3. Click Settings (⚙️)
4. Go to "Advanced" 
5. Click "Choose folder" under "Logseq folder"
6. Select: {}
7. Done! Logseq shows your Brain graph.

No sync needed - same Logseq markdown format!
""".format(brain))


def list_pages(limit: int = 20):
    """List pages in Brain."""
    if bb:
        try:
            pages = bb.get_all_pages(limit=limit)
            if pages:
                print(f"=== Pages (showing {len(pages)} of {len(pages)}) ===")
                for page in pages[:limit]:
                    print(f"  - {page}")
            else:
                print("No pages found. Brain empty or no API access.")
        except Exception as e:
            print(f"Error: {e}")
    else:
        # Fallback to filesystem
        brain = get_brain_path()
        pages_dir = brain / "pages"
        if pages_dir.exists():
            pages = sorted([p.stem for p in pages_dir.rglob("*.md")])
            print(f"=== Pages ({len(pages)}) ===")
            for p in pages[:20]:
                print(f"  - {p}")
        else:
            print("Brain pages directory not found")


def search_brain(query: str):
    """Search Brain pages."""
    if bb:
        try:
            results = bb.search(query, limit=10)
            if results:
                print(f"=== Search: {query} ===")
                for r in results[:10]:
                    print(f"  - {r}")
            else:
                print("No results")
        except Exception as e:
            print(f"Error: {e}")
    else:
        # Fallback
        print("brain_bridge not available")


def journal_today(content: str = ""):
    """Write to today's journal."""
    if bb and content:
        try:
            result = bb.log_to_today_journal(content)
            print(f"Logged to journal: {result}")
        except Exception as e:
            print(f"Error: {e}")
    else:
        print("Usage: makakoo skill logseq journal <content>")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "help"
    args = sys.argv[2:]
    
    if cmd == "status":
        status()
    elif cmd == "connect":
        connect()
    elif cmd == "pages":
        list_pages(int(args[0]) if args else 20)
    elif cmd == "search":
        search_brain(args[0] if args else "")
    elif cmd == "journal":
        journal_today(" ".join(args))
    elif cmd == "help":
        print("""
Logseq Plugin for Makakoo OS

Usage:
    makakoo skill logseq status     # Show brain/logseq status
    makakoo skill logseq connect   # Setup instructions
    makakoo skill logseq pages    # List pages
    makakoo skill logseq search <query>  # Search pages
    makakoo skill logseq journal <content>  # Add to today's journal
    makakoo skill logseq help     # This help
""")
    else:
        status()


if __name__ == "__main__":
    main()
