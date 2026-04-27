#!/usr/bin/env python3
"""
Obsidian plugin for Makakoo OS.

Read/write any Obsidian vault using Logseq markdown format.
Point to any folder with markdown + wikilinks.

Usage:
    makakoo skill obsidian status
    makakoo skill obsidian list [folder]
    makakoo skill obsidian read <name>
    makakoo skill obsidian search <query>
    makakoo skill obsidian create <name> <content>
    makakoo skill obsidian journal <content>
"""

import os
import sys
import json
from pathlib import Path
from datetime import datetime

def get_vault_path() -> Path:
    """Get Obsidian vault path from env or default."""
    return Path(os.environ.get("OBSIDIAN_VAULT_PATH", 
                   Path.home() / "Documents" / "Obsidian Vault"))

def get_brain_path() -> Path:
    """Get Makakoo Brain path."""
    home = Path(os.environ.get("MAKAKOO_HOME", 
                     os.environ.get("HARVEY_HOME", Path.home() / "MAKAKOO")))
    return home / "data" / "Brain"


def status():
    """Show vault status."""
    vault = get_vault_path()
    brain = get_brain_path()
    
    print("=== Obsidian Plugin Status ===\n")
    print(f"Obsidian Vault: {vault}")
    print(f"  Exists: {vault.exists()}")
    
    if vault.exists():
        notes = list(vault.rglob("*.md"))
        print(f"  Notes: {len(notes)}")
        print(f"  Sample: {notes[0].name if notes else 'none'}")
    
    print(f"\nMakakoo Brain: {brain}")
    print(f"  Exists: {brain.exists()}")
    
    if brain.exists():
        pages = list((brain / "pages").rglob("*.md")) if (brain / "pages").exists() else []
        journals = list((brain / "journals").glob("*.md")) if (brain / "journals").exists() else []
        print(f"  Pages: {len(pages)}")
        print(f"  Journals: {len(journals)}")


def list_notes(folder: str = ""):
    """List all notes in vault."""
    vault = get_vault_path()
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        print("Set OBSIDIAN_VAULT_PATH or create ~/Documents/Obsidian Vault")
        return
    
    search = vault / folder if folder else vault
    notes = sorted([str(p.relative_to(vault)) for p in search.rglob("*.md")])
    
    print(f"=== Notes in {vault} ({len(notes)}) ===")
    for n in notes[:50]:
        print(f"  - {n}")
    if len(notes) > 50:
        print(f"  ... and {len(notes) - 50} more")


def read_note(name: str):
    """Read a note by name."""
    vault = get_vault_path()
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        return
    
    # Try exact match first
    for ext in ["", ".md"]:
        path = vault / f"{name}{ext}"
        if path.exists():
            print(path.read_text())
            return
    
    # Try fuzzy search
    name_lower = name.lower()
    for path in vault.rglob("*.md"):
        if name_lower in path.stem.lower():
            print(f"=== {path.name} ===")
            print(path.read_text())
            return
    
    print(f"Note not found: {name}")


def create_note(name: str, content: str = ""):
    """Create a new note."""
    vault = get_vault_path()
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        print("Set OBSIDIAN_VAULT_PATH first")
        return
    
    # Sanitize name
    safe_name = name.replace("/", "-").replace("\\", "-")
    path = vault / f"{safe_name}.md"
    
    if path.exists():
        print(f"Note already exists: {name}")
        return
    
    # Create with Logseq frontmatter
    body = f"""---
type:: page
created:: {datetime.now().isoformat()}
---

# {name}

{content}
"""
    path.write_text(body)
    print(f"Created: {path}")


def search_notes(query: str):
    """Search notes by content."""
    vault = get_vault_path()
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        return
    
    results = []
    query_lower = query.lower()
    
    for path in vault.rglob("*.md"):
        try:
            content = path.read_text().lower()
            if query_lower in content:
                results.append(str(path.relative_to(vault)))
        except:
            pass
    
    if results:
        print(f"=== Search: {query} ({len(results)} results) ===")
        for r in results:
            print(f"  - {r}")
    else:
        print(f"No results for: {query}")


def journal_today(content: str):
    """Add to today's journal in vault."""
    vault = get_vault_path()
    today = datetime.now()
    journal_path = vault / "journals" / f"{today.year}_{today.month:02d}_{today.day:02d}.md"
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        return
    
    journal_path.parent.mkdir(parents=True, exist_ok=True)
    
    entry = f"\n- [[{today.strftime('%H:%M')}]] {content}"
    with open(journal_path, "a") as f:
        f.write(entry)
    
    print(f"Added to journal: {journal_path.name}")


def sync_from_brain():
    """Sync from Makakoo Brain to Obsidian vault."""
    brain = get_brain_path()
    vault = get_vault_path()
    
    if not brain.exists():
        print(f"Brain not found: {brain}")
        return
    
    if not vault.exists():
        print(f"Vault not found: {vault}")
        return
    
    # Sync pages
    brain_pages = brain / "pages"
    if brain_pages.exists():
        for src in brain_pages.rglob("*.md"):
            dst = vault / "brain-sync" / "pages" / src.name
            dst.parent.mkdir(parents=True, exist_ok=True)
            if not dst.exists() or src.stat().st_mtime > dst.stat().st_mtime:
                dst.write_text(src.read_text())
        print(f"Synced {len(list(brain_pages.rglob('*.md')))} pages from Brain to {vault}/brain-sync/pages/")

    # Sync journals
    brain_journals = brain / "journals"
    if brain_journals.exists():
        for src in brain_journals.glob("*.md"):
            dst = vault / "brain-sync" / "journals" / src.name
            dst.parent.mkdir(parents=True, exist_ok=True)
            if not dst.exists() or src.stat().st_mtime > dst.stat().st_mtime:
                dst.write_text(src.read_text())
        print(f"Synced {len(list(brain_journals.glob('*.md')))} journals to {vault}/brain-sync/journals/")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "help"
    args = sys.argv[2:]
    
    if cmd == "status":
        status()
    elif cmd == "list":
        list_notes(args[0] if args else "")
    elif cmd == "read":
        read_note(" ".join(args))
    elif cmd == "create":
        name, content = args[0], " ".join(args[1:]) if len(args) > 1 else ""
        create_note(name, content)
    elif cmd == "search":
        search_notes(" ".join(args))
    elif cmd == "journal":
        journal_today(" ".join(args))
    elif cmd == "sync":
        sync_from_brain()
    elif cmd == "help":
        print("""
Obsidian Plugin for Makakoo OS

Usage:
    makakoo skill obsidian status     # Show vault status
    makakoo skill obsidian list       # List all notes
    makakoo skill obsidian list <folder>  # List in folder
    makakoo skill obsidian read <name>    # Read a note
    makakoo skill obsidian search <query>  # Search notes
    makakoo skill obsidian create <name> <content>  # Create note
    makakoo skill obsidian journal <content>  # Add to today journal
    makakoo skill obsidian sync      # Sync from Brain to vault
    
Set OBSIDIAN_VAULT_PATH to point to your vault.
""")
    else:
        status()


if __name__ == "__main__":
    main()
