#!/usr/bin/env python3
"""
Skill Indexer — Scans all SKILL.md files and generates the skill index.
"""

import argparse
import json
import re
import sys
from datetime import datetime
from pathlib import Path

import yaml

# Post-harvey-os retirement: skills live as plugin dirs under
# $MAKAKOO_HOME/plugins-core/; __file__ parents[2] is now src/ under the
# lib-harvey-core plugin, so resolve MAKAKOO_HOME explicitly.
import os as _os
_MAKAKOO_HOME = Path(
    _os.environ.get("MAKAKOO_HOME")
    or _os.environ.get("HARVEY_HOME")
    or _os.path.expanduser("~/MAKAKOO")
)
HARVEY_OS = _MAKAKOO_HOME / "plugins-core" / "lib-harvey-core"
SKILLS_DIR = _MAKAKOO_HOME / "plugins-core"
REGISTRY_DIR = Path(__file__).parent
INDEX_PATH = REGISTRY_DIR / "skill_index.json"


def extract_trigger_conditions(description: str) -> list[str]:
    """Extract 'Use when...' clauses from description."""
    if not description:
        return []

    # Match "Use when..." patterns in description
    pattern = r"Use when\s+(?:asked to\s+)?[\"']([^\"']+)[\"']|Use when\s+(?:[^.,\n]+[.,\n]?)+"
    matches = []

    # Find quoted phrases in "Use when..." contexts
    use_when_pattern = r"Use when\s+(?:asked to\s+)?[\"']([^\"']+)[\"']"
    for match in re.finditer(use_when_pattern, description, re.IGNORECASE):
        phrase = match.group(1).strip()
        if phrase:
            matches.append(phrase)

    # Also extract comma-separated trigger phrases
    comma_pattern = r"Use when\s+(?:asked to\s+)?[\"']([^\"']+)[\"']\s*,\s*([^\n.]+)"
    for match in re.finditer(comma_pattern, description, re.IGNORECASE):
        for group in match.groups():
            if group:
                # Split on commas and "or"
                parts = re.split(r',\s*(?:or\s+)?|\s+or\s+', group.strip())
                for part in parts:
                    part = part.strip().strip('"\' ')
                    if part and len(part) > 2:
                        matches.append(part)

    # Fallback: extract any quoted phrases that look like trigger conditions
    if not matches:
        quote_pattern = r"[\"']([^\"']{5,50})[\"']"
        for match in re.finditer(quote_pattern, description):
            phrase = match.group(1).strip()
            # Filter to likely trigger phrases
            if phrase and not phrase.startswith("Use when"):
                # Skip if it looks like a code snippet or path
                if not phrase.startswith("/") and not phrase.startswith("$"):
                    matches.append(phrase)

    # Deduplicate while preserving order
    seen = set()
    unique = []
    for m in matches:
        m_lower = m.lower()
        if m_lower not in seen:
            seen.add(m_lower)
            unique.append(m)

    return unique


def extract_keywords_from_description(description: str) -> list[str]:
    """Extract meaningful keywords from description for semantic scoring."""
    if not description:
        return []

    # Remove common stopwords
    stopwords = {
        'a', 'an', 'the', 'and', 'or', 'but', 'in', 'on', 'at', 'to', 'for',
        'of', 'with', 'by', 'from', 'as', 'is', 'was', 'are', 'were', 'been',
        'be', 'have', 'has', 'had', 'do', 'does', 'did', 'will', 'would',
        'could', 'should', 'may', 'might', 'must', 'shall', 'can', 'need',
        'that', 'which', 'who', 'whom', 'this', 'these', 'those', 'it', 'its',
        'not', 'no', 'nor', 'either', 'neither', 'both', 'each', 'every',
        'all', 'any', 'some', 'most', 'few', 'more', 'most', 'less', 'least',
        'very', 'really', 'just', 'only', 'even', 'still', 'also', 'however',
        'than', 'then', 'so', 'because', 'since', 'unless', 'until', 'when',
        'where', 'why', 'how', 'what', 'which', 'who', 'whom', 'whose',
    }

    # Clean description - remove code blocks and special chars
    cleaned = re.sub(r'```[\s\S]*?```', '', description)
    cleaned = re.sub(r'`[^`]+`', '', cleaned)
    cleaned = re.sub(r'[^\w\s]', ' ', cleaned)

    words = cleaned.lower().split()
    keywords = [w for w in words if w not in stopwords and len(w) > 2]

    return keywords


def parse_frontmatter(content: str) -> dict:
    """Parse YAML frontmatter from SKILL.md content."""
    match = re.match(r'^---\n(.*?)\n---', content, re.DOTALL)
    if match:
        try:
            return yaml.safe_load(match.group(1)) or {}
        except yaml.YAMLError:
            return {}
    return {}


def index_all_skills() -> dict:
    """Scan all SKILL.md files and build the index."""
    index = {
        "generated_at": datetime.utcnow().isoformat() + "Z",
        "skills": [],
        "categories": {},
    }

    for skill_path in sorted(SKILLS_DIR.rglob("SKILL.md")):
        # Skip _registry directory itself
        if "_registry" in skill_path.parts:
            continue

        # Determine category from parent directory
        parts = skill_path.relative_to(SKILLS_DIR).parts
        category = parts[0] if len(parts) > 1 else "unknown"

        # Parse frontmatter
        try:
            content = skill_path.read_text(encoding="utf-8")
        except Exception as e:
            print(f"Warning: Could not read {skill_path}: {e}", file=sys.stderr)
            continue

        frontmatter = parse_frontmatter(content)

        name = frontmatter.get("name") or skill_path.stem
        description = frontmatter.get("description", "")

        # Extract description text (may be multiline)
        if isinstance(description, list):
            description = " ".join(str(d) for d in description)

        # Get hermes metadata
        metadata = frontmatter.get("metadata", {}) or {}
        hermes = metadata.get("hermes", {}) or {}

        # Extract fields
        tags = hermes.get("tags", [])
        related_skills = hermes.get("related_skills", [])
        version = frontmatter.get("version", "1.0.0")
        dependencies = frontmatter.get("dependencies", [])

        # Extract trigger conditions from description
        trigger_conditions = extract_trigger_conditions(description)

        # Extract keywords for semantic matching
        keywords = extract_keywords_from_description(description)

        # Build skill entry
        skill_entry = {
            "name": name,
            "description": description,
            "path": str(skill_path.relative_to(HARVEY_OS)),
            "category": category,
            "version": version,
            "dependencies": dependencies,
            "tags": tags,
            "related_skills": related_skills,
            "trigger_conditions": trigger_conditions,
            "keywords": keywords,
        }

        index["skills"].append(skill_entry)

        # Index by category
        if category not in index["categories"]:
            index["categories"][category] = []
        index["categories"][category].append(name)

    return index


def sync_skills_to_gsd_todos(
    skills: list[dict],
    baseline: bool = False,
    dry_run: bool = False,
) -> int:
    """
    Sync newly discovered skills to GSD todo system.

    For each skill that has no corresponding todo in .planning/todos/pending/,
    create a todo capturing: name, category, description, trigger phrases, deps.

    Args:
        skills: list of skill dicts from the index
        baseline: if True, create todos for ALL skills (initial backlog population).
                  if False, only create for skills with no implementation files.
        dry_run: if True, don't write anything, just report what would happen.

    Returns the number of (would-be or actual) todos.
    """
    import json
    from datetime import datetime

    HARVEY_ROOT = Path(__file__).resolve().parents[3]  # ~/MAKAKOO
    SKILLS_ROOT = HARVEY_ROOT / "plugins-core"
    TODOS_DIR = HARVEY_ROOT / ".planning" / "todos" / "pending"
    TODOS_DIR.mkdir(parents=True, exist_ok=True)

    # Load existing todo titles to avoid duplicates
    existing_titles = {}  # lowercase title -> filename
    if TODOS_DIR.exists():
        for todo_file in TODOS_DIR.glob("*.md"):
            try:
                content = todo_file.read_text()
                for line in content.split("\n"):
                    if line.startswith("title:"):
                        # Strip YAML quotes and normalize
                        t = line.replace("title:", "").strip().strip('"').strip("'").lower()
                        existing_titles[t] = todo_file.name
                        break
            except Exception:
                pass

    def skill_is_implemented(skill: dict) -> bool:
        """Return True if skill directory has actual implementation files."""
        # Use the path from the index — it reflects the actual directory
        skill_rel = skill.get("path", "")
        if skill_rel:
            skill_path = HARVEY_ROOT / skill_rel
        else:
            skill_path = SKILLS_ROOT / skill["category"] / skill["name"]
        if not skill_path.exists() or not skill_path.is_dir():
            return False
        # Check for .py, .sh, .js, .ts files (not just SKILL.md)
        for ext in ("*.py", "*.sh", "*.js", "*.ts"):
            if list(skill_path.glob(ext)):
                return True
        return False

    checked = would_create = 0
    today = datetime.utcnow().strftime("%Y-%m-%d")
    timestamp = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")

    for skill in skills:
        checked += 1
        title_lower = skill["name"].lower()
        title_slug = title_lower.replace("/", "-").replace("_", "-")

        # Skip if already have a todo for this skill
        if title_lower in existing_titles or title_slug in existing_titles:
            continue

        # In non-baseline mode, skip skills that have implementation
        if not baseline and skill_is_implemented(skill):
            continue

        would_create += 1

        # Build todo content
        description = skill.get("description", "")[:200]
        if len(skill.get("description", "")) > 200:
            description += "..."

        triggers = skill.get("trigger_conditions", [])
        trigger_text = ""
        if triggers:
            trigger_text = "\n### Trigger Phrases\n"
            for t in triggers[:5]:
                trigger_text += f"- {t}\n"

        deps = skill.get("dependencies", [])
        dep_text = ""
        if deps:
            dep_text = "\n### Dependencies\n"
            for d in deps:
                dep_text += f"- {d}\n"

        tags = skill.get("tags", [])
        tags_text = ""
        if tags:
            tags_text = "\n### Tags\n"
            for tag in tags[:8]:
                tags_text += f"- {tag}\n"

        content = f"""---
created: {timestamp}
title: "{skill["name"]}"
area: "{skill["category"]}"
files:
  - "plugins-core/{skill["category"]}/{skill["name"]}/"
---

## Problem

Skill "{skill["name"]}" in category "{skill["category"]}" needs implementation.

### Description

{description}

{trigger_text}{dep_text}{tags_text}
## Solution

TBD — implement the skill following the SKILL.md template.
"""

        filename = f"{today}-skill-{title_slug}.md"
        filepath = TODOS_DIR / filename

        if dry_run:
            status = "IMPLEMENTED" if skill_is_implemented(skill) else "needs impl"
            print(f"  [{status}] {skill['name']} ({skill['category']})")
        else:
            filepath.write_text(content)
            print(f"  Created: {filename}")

    print(f"\nChecked {checked} skills, {would_create} need todos", end="")
    if dry_run:
        print(" (dry run — no files written)", end="")
    print()

    return would_create


def main():
    """Generate the skill index and save to JSON."""
    parser = argparse.ArgumentParser(description="Skill indexer")
    parser.add_argument(
        "--reindex-embeddings",
        action="store_true",
        help="Force re-embedding all skills in Chroma"
    )
    parser.add_argument(
        "--sync-gsd-todos",
        action="store_true",
        help="Sync new skills to GSD todo system"
    )
    parser.add_argument(
        "--baseline",
        action="store_true",
        help="With --sync-gsd-todos: create todos for ALL skills (not just unimpl ones)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Used with --sync-gsd-todos to show what would be created"
    )
    args = parser.parse_args()

    print(f"Indexing skills in {SKILLS_DIR}...")

    index = index_all_skills()

    # Save to JSON
    INDEX_PATH.parent.mkdir(parents=True, exist_ok=True)
    INDEX_PATH.write_text(json.dumps(index, indent=2))

    print(f"Indexed {len(index['skills'])} skills across {len(index['categories'])} categories")
    print(f"Index saved to {INDEX_PATH}")

    # Show categories
    for cat, skills in sorted(index["categories"].items()):
        print(f"  {cat}: {len(skills)} skills")

    # Index embeddings in Chroma if requested
    if args.reindex_embeddings:
        print("\nIndexing embeddings in Chroma...")
        try:
            sys.path.insert(0, str(REGISTRY_DIR))
            from embedding_service import get_instance
            svc = get_instance()
            count = svc.embed_and_index_skills(index["skills"])
            print(f"Indexed {count} skills in Chroma")
        except Exception as e:
            print(f"Warning: Could not index embeddings: {e}", file=sys.stderr)

    # Sync new skills to GSD todos if requested
    if args.sync_gsd_todos:
        print("\nSyncing new skills to GSD todos...")
        would_create = sync_skills_to_gsd_todos(
            index["skills"],
            baseline=args.baseline,
            dry_run=args.dry_run,
        )
        print(f"Result: {would_create} todos {('(dry run)' if args.dry_run else '(written)')}")


if __name__ == "__main__":
    main()
