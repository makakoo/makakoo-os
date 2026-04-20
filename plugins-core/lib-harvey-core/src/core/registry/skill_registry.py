#!/usr/bin/env python3
"""
Skill Registry — Loads and watches the skill index, provides discovery API.

Usage:
    python3 skill_registry.py "review my PR and check emails"
    python3 skill_registry.py --rebuild
    python3 skill_registry.py --list-categories
"""

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Optional

# Add registry dir to path for imports
REGISTRY_DIR = Path(__file__).parent
# Post-harvey-os retirement (2026-04-20): skills are plugin dirs under
# $MAKAKOO_HOME/plugins-core/ (and $MAKAKOO_HOME/plugins once installed).
_MAKAKOO_HOME = Path(
    os.environ.get("MAKAKOO_HOME")
    or os.environ.get("HARVEY_HOME")
    or os.path.expanduser("~/MAKAKOO")
)
HARVEY_OS = _MAKAKOO_HOME / "plugins-core" / "lib-harvey-core"  # legacy name, used as read root
SKILLS_DIR = _MAKAKOO_HOME / "plugins-core"
INDEX_PATH = REGISTRY_DIR / "skill_index.json"

# Import our modules
sys.path.insert(0, str(REGISTRY_DIR))
from skill_indexer import index_all_skills
from intent_matcher import match_skills, decompose_request, resolve_dependencies


class SkillRegistry:
    """
    Skill registry with auto-rebuild on skills/ mtime change.
    """

    def __init__(self, index_path: Path = None, skills_dir: Path = None):
        self.index_path = index_path or INDEX_PATH
        self.skills_dir = skills_dir or SKILLS_DIR
        self._index = None
        self._skills_mtime = None
        self._load()

    def _load(self):
        """Load the index from disk, or rebuild if missing/stale."""
        skills_mtime = self._get_skills_mtime()

        # Check if index exists and is newer than skills dir
        if self.index_path.exists():
            index_mtime = self.index_path.stat().st_mtime
            if skills_mtime and skills_mtime <= index_mtime:
                try:
                    self._index = json.loads(self.index_path.read_text())
                    self._skills_mtime = skills_mtime
                    return
                except json.JSONDecodeError:
                    pass

        # Rebuild index
        self.rebuild()

    def _get_skills_mtime(self) -> float:
        """Get max mtime of all SKILL.md files."""
        max_mtime = 0.0
        for skill_path in self.skills_dir.rglob("SKILL.md"):
            if "_registry" in skill_path.parts:
                continue
            try:
                mtime = skill_path.stat().st_mtime
                if mtime > max_mtime:
                    max_mtime = mtime
            except OSError:
                pass
        return max_mtime

    def rebuild(self):
        """Rebuild the index from disk."""
        print(f"Rebuilding skill index from {self.skills_dir}...")
        self._index = index_all_skills()
        self._skills_mtime = self._get_skills_mtime()

        # Save to disk
        self.index_path.parent.mkdir(parents=True, exist_ok=True)
        self.index_path.write_text(json.dumps(self._index, indent=2))
        print(f"Index saved to {self.index_path}")

    def check_stale(self) -> bool:
        """Check if the index is stale and needs rebuild."""
        current_mtime = self._get_skills_mtime()
        return current_mtime > (self._skills_mtime or 0)

    def ensure_current(self):
        """Rebuild if index is stale."""
        if self.check_stale():
            self.rebuild()

    @property
    def skills(self) -> list[dict]:
        """Get all indexed skills."""
        self.ensure_current()
        return self._index.get("skills", [])

    @property
    def categories(self) -> dict:
        """Get category -> skills mapping."""
        self.ensure_current()
        return self._index.get("categories", {})

    def find_skills(self, query: str, top_k: int = 5) -> list[dict]:
        """
        Find skills matching a query.

        Returns list of dicts with 'skill' and 'scores' keys.
        """
        self.ensure_current()

        # Load category boost config
        category_boost_path = REGISTRY_DIR / "CATEGORY_BOOST.json"
        if category_boost_path.exists():
            category_config = json.loads(category_boost_path.read_text())
        else:
            category_config = {}

        return match_skills(
            query=query,
            skills=self.skills,
            category_config=category_config,
            top_k=top_k,
        )

    def get_skill(self, name: str) -> Optional[dict]:
        """Get a specific skill by name."""
        self.ensure_current()
        for skill in self.skills:
            if skill["name"] == name:
                return skill
        return None

    def list_categories(self) -> list[str]:
        """List all categories."""
        return list(self.categories.keys())

    def skills_in_category(self, category: str) -> list[str]:
        """List all skills in a category."""
        return self.categories.get(category, [])


def print_match_results(results: list[dict], verbose: bool = False):
    """Print match results in a readable format."""
    if not results:
        print("  No matches found.")
        return

    for i, item in enumerate(results, 1):
        skill = item["skill"]
        scores = item["scores"]

        print(f"\n  {i}. {skill['name']} ({skill['category']})")
        print(f"     Score: {scores['total']:.3f}")

        if verbose:
            print(f"     Phrase match: {scores['phrase_match']:.3f}")
            print(f"     Semantic:     {scores['semantic']:.3f}")
            print(f"     Tag overlap:  {scores['tag_overlap']:.3f}")
            print(f"     Category:     {scores['category_boost']:.3f}")

        desc = skill.get("description", "")[:120]
        if len(desc) == 120:
            desc += "..."
        print(f"     {desc}")

        if skill.get("tags"):
            print(f"     Tags: {', '.join(skill['tags'][:5])}")


def main():
    parser = argparse.ArgumentParser(
        prog="skill_registry.py",
        description="Harvey Skill Registry CLI",
    )
    parser.add_argument(
        "query",
        nargs="?",
        help="Query to match skills against",
    )
    parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Force rebuild of the skill index",
    )
    parser.add_argument(
        "--list-categories",
        action="store_true",
        help="List all categories",
    )
    parser.add_argument(
        "--category",
        dest="show_category",
        metavar="CATEGORY",
        help="Show skills in a category",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show detailed scores",
    )
    parser.add_argument(
        "--top-k",
        type=int,
        default=5,
        help="Number of results to return (default: 5)",
    )
    parser.add_argument(
        "--decompose",
        action="store_true",
        help="Show how query would be decomposed",
    )
    parser.add_argument(
        "--resolve-deps",
        metavar="SKILL",
        nargs="+",
        help="Resolve dependencies for given skill(s)",
    )

    args = parser.parse_args()

    registry = SkillRegistry()

    # Rebuild if requested
    if args.rebuild:
        registry.rebuild()
        print(f"Rebuilt index: {len(registry.skills)} skills")
        return

    # List categories
    if args.list_categories:
        print("Categories:")
        for cat in sorted(registry.list_categories()):
            count = len(registry.skills_in_category(cat))
            print(f"  {cat}: {count} skills")
        return

    # Show skills in category
    if args.show_category:
        skills = registry.skills_in_category(args.show_category)
        if not skills:
            print(f"Unknown category: {args.show_category}")
            print(f"Available: {', '.join(sorted(registry.list_categories()))}")
            return
        print(f"Skills in '{args.show_category}':")
        for s in skills:
            print(f"  - {s}")
        return

    # Resolve dependencies
    if args.resolve_deps:
        resolved = resolve_dependencies(list(args.resolve_deps), registry.skills)
        print(f"Resolved dependencies for {args.resolve_deps}:")
        print(f"  Order to load: {' -> '.join(resolved)}")
        return

    # Decompose query without matching
    if args.decompose and args.query:
        segments = decompose_request(args.query)
        print(f"Query decomposition for '{args.query}':")
        for i, seg in enumerate(segments, 1):
            print(f"  {i}. {seg}")
        return

    # Match skills
    if args.query:
        print(f'\nMatching skills for: "{args.query}"')
        print("=" * 60)

        # Show decomposition
        segments = decompose_request(args.query)
        if len(segments) > 1:
            print(f"\nDecomposed into {len(segments)} segments:")
            for i, seg in enumerate(segments, 1):
                print(f"  {i}. {seg}")
            print()

        # Match each segment
        for seg in segments:
            seg_matches = registry.find_skills(seg, top_k=args.top_k)
            print(f'\nMatches for: "{seg}"')
            print_match_results(seg_matches, verbose=args.verbose)

        # Show resolved dependencies for top match
        all_top = []
        for seg in segments:
            matches = registry.find_skills(seg, top_k=1)
            if matches:
                all_top.append(matches[0]["skill"]["name"])

        if all_top:
            print(f"\n{'=' * 60}")
            resolved = resolve_dependencies(all_top, registry.skills)
            if len(resolved) > len(all_top):
                print(f"With dependencies resolved: {' -> '.join(resolved)}")

        return

    # No query - show help
    parser.print_help()
    print("\n\nExamples:")
    print('  python3 skill_registry.py "review my PR"')
    print('  python3 skill_registry.py "review my PR and check emails" --verbose')
    print("  python3 skill_registry.py --list-categories")
    print("  python3 skill_registry.py --category dev")
    print('  python3 skill_registry.py --decompose "review PR and send email"')
    print("  python3 skill_registry.py --resolve-deps review plan")


# ---------------------------------------------------------------------------
# Skill Frontmatter Parsing
# ---------------------------------------------------------------------------

import re
from dataclasses import dataclass, field
from typing import Optional


SKILL_FRONTMATTER_SCHEMA = re.compile(
    r"^---\s*\n"
    r"(.*?)"
    r"\n---\s*\n",
    re.DOTALL | re.MULTILINE,
)


@dataclass
class SkillDef:
    """
    A discovered skill definition with parsed frontmatter.

    Attributes:
        name: Skill name (from frontmatter)
        description: Brief description
        path: Path to the SKILL.md file
        allowed_tools: List of allowed tool names
        when_to_use: When to use this skill
        argument_hint: Hint for arguments
        arguments: Full arguments specification
        context: Execution context (fork/continue/solo)
        agent: Agent type
        content: Full SKILL.md content
    """

    name: str
    description: str
    path: Path
    allowed_tools: list[str] = field(default_factory=list)
    when_to_use: str = ""
    argument_hint: str = ""
    arguments: str = ""
    context: str = "fork"
    agent: str = "general-purpose"
    content: str = ""


def parse_skill_frontmatter(content: str) -> Optional[dict]:
    """
    Parse SKILL.md frontmatter into a dict.

    Args:
        content: Full SKILL.md file content

    Returns:
        Dict with frontmatter fields or None if no frontmatter found

    Frontmatter schema:
        name: skill-name
        description: Brief description
        allowed-tools:
          - Tool1
          - Tool2
        when_to_use: |
          When to use this skill
        argument-hint: [arg1, arg2]
        arguments: |
          - name: arg1
            description: ...
        context: fork
        agent: general-purpose
    """
    match = SKILL_FRONTMATTER_SCHEMA.search(content)
    if not match:
        return None

    frontmatter_text = match.group(1)
    result: dict = {}

    # Split into lines and process
    lines = frontmatter_text.split("\n")
    i = 0

    while i < len(lines):
        line = lines[i]

        # Check for array items under current key (indented with "- ")
        stripped = line.strip()
        if stripped.startswith("- "):
            # This is an array item
            item_value = stripped[1:].strip()
            if result.get("_current_key") and isinstance(
                result.get(result["_current_key"]), list
            ):
                result[result["_current_key"]].append(item_value)
            i += 1
            continue

        # Check for key: value
        key_match = re.match(r"^(\w+(?:-\w+)*):\s*(.*)$", line)
        if key_match:
            key = key_match.group(1)
            dict_key = key.replace("-", "_")
            raw_value = key_match.group(2).strip()

            # Check if this is the start of a block array (next lines are "- item")
            if raw_value == "" or raw_value == "|":
                # Check if next non-empty line is an array item
                next_idx = i + 1
                while next_idx < len(lines) and lines[next_idx].strip() == "":
                    next_idx += 1
                if next_idx < len(lines) and lines[next_idx].strip().startswith("- "):
                    # It's a block array
                    result[dict_key] = []
                    result["_current_key"] = dict_key
                    i = next_idx
                    continue

            # Handle pipe multiline
            if raw_value.startswith("|"):
                # Multiline text follows
                multiline = raw_value[1:].strip() + "\n"
                i += 1
                while i < len(lines):
                    next_line = lines[i]
                    # Check if next line is a continuation (indented or empty)
                    if next_line.startswith("  ") or (next_line.strip() == ""):
                        if next_line.strip() == "":
                            multiline += "\n"
                        else:
                            multiline += next_line.strip() + "\n"
                        i += 1
                    else:
                        break
                result[dict_key] = multiline.rstrip("\n")
                continue

            # Handle inline JSON array
            if raw_value.startswith("["):
                try:
                    result[dict_key] = json.loads(raw_value)
                except json.JSONDecodeError:
                    result[dict_key] = [raw_value]
                i += 1
                continue

            # Simple value
            result[dict_key] = raw_value
            result["_current_key"] = dict_key
            i += 1
            continue

        # Empty line or continuation
        if line.strip() == "":
            result["_current_key"] = None
        i += 1

    # Clean up internal key
    result.pop("_current_key", None)

    # Post-process: ensure allowed_tools is a list
    if "allowed_tools" in result and not isinstance(result["allowed_tools"], list):
        result["allowed_tools"] = [result["allowed_tools"]]

    return result


def cached_read_skill(path: Path) -> bytes:
    """
    Read a SKILL.md file using FileStateCache if available.

    Falls back to direct read if cache not available.
    """
    try:
        from harvey_os.core.indexing.file_state_cache import get_file_cache

        cache = get_file_cache()
        return cache.read(str(path))
    except (ImportError, Exception):
        # Fallback to direct read
        return path.read_bytes()


def discover_skills(
    search_dirs: list[Path],
    max_depth: int = 3,
    skip_patterns: Optional[list[str]] = None,
) -> list[SkillDef]:
    """
    Discover skills by walking directories and parsing SKILL.md files.

    Args:
        search_dirs: List of directory paths to search
        max_depth: Maximum directory depth to search
        skip_patterns: List of path patterns to skip (default: ["_registry", "__pycache__"])

    Returns:
        List of SkillDef objects with parsed frontmatter

    Search directories:
        - $HARVEY_HOME/plugins-core/
        - $HARVEY_HOME/agents/*/ (each agent may have skills)
        - ~/.harvey/skills/ (user-installed skills)
    """
    if skip_patterns is None:
        skip_patterns = ["_registry", "__pycache__", ".git", "node_modules"]

    discovered: list[SkillDef] = []

    for search_dir in search_dirs:
        if not search_dir.exists():
            continue

        for skillyml in search_dir.rglob("SKILL.md"):
            # Check depth
            try:
                depth = len(skillyml.relative_to(search_dir).parts) - 1
                if depth > max_depth:
                    continue
            except ValueError:
                pass

            # Check skip patterns
            if any(pattern in skillyml.parts for pattern in skip_patterns):
                continue

            # Check file size (< 100KB)
            try:
                size = skillyml.stat().st_size
                if size > 100 * 1024:
                    continue
            except OSError:
                continue

            # Read and parse
            try:
                content_bytes = cached_read_skill(skillyml)
                content = content_bytes.decode("utf-8", errors="replace")
                frontmatter = parse_skill_frontmatter(content)

                if frontmatter is None:
                    continue

                skill_def = SkillDef(
                    name=frontmatter.get("name", skillyml.stem),
                    description=frontmatter.get("description", ""),
                    path=skillyml,
                    allowed_tools=frontmatter.get("allowed_tools", []),
                    when_to_use=frontmatter.get("when_to_use", ""),
                    argument_hint=frontmatter.get("argument_hint", ""),
                    arguments=frontmatter.get("arguments", ""),
                    context=frontmatter.get("context", "fork"),
                    agent=frontmatter.get("agent", "general-purpose"),
                    content=content,
                )
                discovered.append(skill_def)

            except Exception:
                # Skip files that can't be read/parsed
                continue

    return discovered


def get_all_skills_enhanced(
    harvey_home: Optional[Path] = None,
) -> list[SkillDef]:
    """
    Enhanced get_all_skills using frontmatter parsing.

    Searches in:
        - {harvey_home}/plugins-core/
        - {harvey_home}/agents/*/ (agent skills)
        - {harvey_home}/.harvey/skills/ (user skills)
        - ~/.harvey/skills/ (global user skills)

    Args:
        harvey_home: Harvey home directory (default: from HARVEY_HOME env or ~)

    Returns:
        List of all discovered SkillDef objects
    """
    import os

    if harvey_home is None:
        harvey_home = Path(os.environ.get("HARVEY_HOME", str(Path.home() / "HARVEY")))

    search_dirs = [
        harvey_home / "plugins-core",
        harvey_home / "agents",
        harvey_home / ".harvey" / "skills",
        Path.home() / ".harvey" / "skills",
    ]

    return discover_skills(search_dirs, max_depth=4)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    main()
