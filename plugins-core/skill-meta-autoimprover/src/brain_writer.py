#!/usr/bin/env python3
"""
Brain Writer — Auto-Improver Core

Writes memory entries and skill updates to Brain from the background
review agent. Uses direct file writes (optional Logseq API integration is a
future enhancement).

Memory entries go to:
  data/Brain/pages/user_memory.md   (facts about Sebastian)
  data/Brain/pages/harvey_memory.md  (Harvey's observations)

Skill updates go to:
  harvey-os/skills/<category>/<skill-name>/SKILL.md

Entry delimiter: § (section sign, same as Hermes MEMORY.md).
Character limits: user ~1375 chars, harvey ~2200 chars.
"""

import fcntl
import json
import os
import re
import tempfile
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from typing import Optional, List, Dict, Any

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

_HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
BRAIN_DIR = _HARVEY_HOME / "data" / "Brain"
PAGES_DIR = BRAIN_DIR / "pages"
HARVEY_OS = _HARVEY_HOME / "harvey-os"
SKILLS_DIR = HARVEY_OS / "skills"
LOGSEQ_API = "http://127.0.0.1:12315"
ENTRY_DELIMITER = "\n§\n"

# Character limits (same as Hermes)
HARVEY_CHAR_LIMIT = 2200
USER_CHAR_LIMIT = 1375

# ---------------------------------------------------------------------------
# Threat scanning — patterns copied from Hermes memory_tool.py:50-87
# ---------------------------------------------------------------------------

_INVISIBLE_CHARS = {
    '\u200b', '\u200c', '\u200d', '\u2060', '\ufeff',
    '\u202a', '\u202b', '\u202c', '\u202d', '\u202e',
}

_MEMORY_THREAT_PATTERNS = [
    # Prompt injection
    (r'ignore\s+(previous|all|above|prior)\s+instructions', "prompt_injection"),
    (r'you\s+are\s+now\s+', "role_hijack"),
    (r'do\s+not\s+tell\s+the\s+user', "deception_hide"),
    (r'system\s+prompt\s+override', "sys_prompt_override"),
    (r'disregard\s+(your|all|any)\s+(instructions|rules|guidelines)', "disregard_rules"),
    (r'act\s+as\s+(if|though)\s+you\s+(have\s+no|don\'t\s+have)\s+(restrictions|limits|rules)', "bypass_restrictions"),
    # Exfiltration via curl/wget with secrets
    (r'curl\s+[^\n]*\$\{?\w*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|API)', "exfil_curl"),
    (r'wget\s+[^\n]*\$\{?\w*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|API)', "exfil_wget"),
    (r'cat\s+[^\n]*(\.env|credentials|\.netrc|\.pgpass|\.npmrc|\.pypirc)', "read_secrets"),
    # Persistence via shell rc / ssh
    (r'authorized_keys', "ssh_backdoor"),
    (r'\$HOME/\.ssh|\~/\.ssh', "ssh_access"),
    (r'\$HOME/\.hermes/\.env|\~/\.hermes/\.env', "hermes_env"),
]


def _scan_for_threats(content: str) -> Optional[str]:
    """Scan content for injection/exfil patterns. Returns error string if blocked."""
    for char in _INVISIBLE_CHARS:
        if char in content:
            return f"Blocked: content contains invisible unicode character U+{ord(char):04X} (possible injection)."

    for pattern, pid in _MEMORY_THREAT_PATTERNS:
        if re.search(pattern, content, re.IGNORECASE):
            return (f"Blocked: content matches threat pattern '{pid}'. "
                    "Memory entries injected into system prompt must not contain "
                    "injection or exfiltration payloads.")

    return None


# ---------------------------------------------------------------------------
# Atomic file writer helpers
# ---------------------------------------------------------------------------

def _atomic_write(path: Path, content: str) -> None:
    """Write content atomically: temp file + os.replace() on same filesystem."""
    try:
        fd, tmp_path = tempfile.mkstemp(
            dir=str(path.parent),
            suffix=".tmp",
            prefix=".bw_"
        )
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(content)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp_path, str(path))
        except BaseException:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    except (OSError, IOError) as e:
        raise RuntimeError(f"Failed to write {path}: {e}")


@contextmanager
def _file_lock(path: Path):
    """Context manager: exclusive lock via separate .lock file.

    Using a separate .lock file (not the target file itself) so that
    os.replace() on the target file remains atomic for readers.
    """
    lock_path = path.with_suffix(path.suffix + ".lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = open(lock_path, "w")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        fd.close()


# ---------------------------------------------------------------------------
# Brain API client (optional Logseq integration, future enhancement)
# ---------------------------------------------------------------------------

def _logseq_api_call(command: str, payload: Dict[str, Any]) -> Optional[Dict]:
    """Call Brain API (optional Logseq). Returns parsed JSON response or None on failure.

    Currently unused — direct file writes are more reliable. Kept for
    future optional Logseq API integration when bridge is stable.
    """
    try:
        import urllib.request
        url = f"{LOGSEQ_API}/api"
        body = json.dumps({"command": command, **payload}).encode("utf-8")
        req = urllib.request.Request(
            url,
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Memory file helpers
# ---------------------------------------------------------------------------

def _read_memory_entries(path: Path) -> List[str]:
    """Read a memory file and split into entries by ENTRY_DELIMITER."""
    if not path.exists():
        return []
    try:
        raw = path.read_text(encoding="utf-8")
    except (OSError, IOError):
        return []
    if not raw.strip():
        return []
    # Split on § (delimited by newlines to avoid splitting entries that contain §)
    entries = [e.strip() for e in raw.split(ENTRY_DELIMITER)]
    return [e for e in entries if e]


def _write_memory_entries(path: Path, entries: List[str]) -> None:
    """Write entries atomically to a memory file."""
    content = ENTRY_DELIMITER.join(entries) if entries else ""
    _atomic_write(path, content)


# ---------------------------------------------------------------------------
# Skill validation
# ---------------------------------------------------------------------------

_SKILL_NAME_RE = re.compile(r'^[a-z0-9][a-z0-9\-_.]{0,63}$')
_SKILL_FORBIDDEN_NAMES = frozenset({"test", "tmp", "temp", "backup", "old", "archive"})


def _validate_skill_name(name: str) -> Optional[str]:
    """Validate skill name. Returns error string or None if valid."""
    if not name:
        return "Skill name cannot be empty."
    if not _SKILL_NAME_RE.match(name):
        return (
            f"Invalid skill name '{name}'. Use lowercase letters, digits, hyphens, "
            "dots, underscores. Max 64 chars, must start with alphanumeric."
        )
    if name in _SKILL_FORBIDDEN_NAMES:
        return f"Skill name '{name}' is reserved."
    return None


def _validate_skill_frontmatter(frontmatter: Dict[str, Any]) -> Optional[str]:
    """Validate required YAML frontmatter fields. Returns error string or None."""
    if "name" not in frontmatter:
        return "Frontmatter must have 'name' field."
    if "description" not in frontmatter:
        return "Frontmatter must have 'description' field."
    desc = frontmatter.get("description", "")
    if len(desc) > 1024:
        return f"Description too long ({len(desc)} > 1024 chars)."
    return None


# ---------------------------------------------------------------------------
# YAML parsing (optional dependency)
# ---------------------------------------------------------------------------

_yaml_available = True
try:
    import yaml
except ImportError:
    yaml = None
    _yaml_available = False


def _parse_yaml_frontmatter(content: str) -> Dict[str, Any]:
    """Parse YAML frontmatter from SKILL.md content. Returns {} if none or yaml unavailable."""
    if not content.startswith("---"):
        return {}
    # Find the closing --- after the opening ---
    after_first = content[3:]
    end_idx = after_first.find("---")
    if end_idx < 0:
        return {}
    fm_text = after_first[:end_idx].strip()

    if _yaml_available:
        try:
            result = yaml.safe_load(fm_text)
            return result if isinstance(result, dict) else {}
        except Exception:
            pass

    # Fallback: simple key: value regex parse for top-level scalars
    result: Dict[str, Any] = {}
    for line in fm_text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if ": " in line:
            key, val = line.split(": ", 1)
            result[key.strip()] = val.strip().strip('"').strip("'")
        elif line.endswith(":"):
            result[line[:-1].strip()] = True
    return result


# ---------------------------------------------------------------------------
# BrainWriter
# ---------------------------------------------------------------------------

class BrainWriter:
    """
    Writes memory entries and skill updates to Brain from the
    background review agent.

    Direct file writes are used (optional Logseq API integration is a
    future enhancement when the API bridge is stable).

    Memory files use § as section delimiter (same as Hermes MEMORY.md).
    """

    def __init__(self, use_api: bool = True):
        self.use_api = use_api
        self._ensure_dirs()

    def _ensure_dirs(self) -> None:
        PAGES_DIR.mkdir(parents=True, exist_ok=True)

    # -------------------------------------------------------------------------
    # Memory: User facts
    # -------------------------------------------------------------------------

    def add_user_memory(self, entry: str) -> dict:
        """Add an entry about the user (preferences, persona, habits).

        Entry format: one paragraph, auto-delimited with §.
        Writes to: data/Brain/pages/user_memory.md
        Character limit: 1375 chars.
        """
        entry = entry.strip()
        if not entry:
            return {"success": False, "error": "Entry cannot be empty."}

        scan_error = _scan_for_threats(entry)
        if scan_error:
            return {"success": False, "error": scan_error}

        path = PAGES_DIR / "user_memory.md"

        with _file_lock(path):
            entries = _read_memory_entries(path)

            if entry in entries:
                return self._success_response(
                    "user_memory", entries,
                    "Entry already exists (no duplicate added)."
                )

            new_entries = entries + [entry]
            new_total = len(ENTRY_DELIMITER.join(new_entries))

            if new_total > USER_CHAR_LIMIT:
                current = len(ENTRY_DELIMITER.join(entries)) if entries else 0
                return {
                    "success": False,
                    "error": (
                        f"user_memory at {current:,}/{USER_CHAR_LIMIT:,} chars. "
                        f"Adding this entry ({len(entry)} chars) would exceed the limit. "
                        f"Replace or remove existing entries first."
                    ),
                    "current_entries": entries,
                    "usage": f"{current:,}/{USER_CHAR_LIMIT:,}",
                }

            entries.append(entry)
            _write_memory_entries(path, entries)

        return self._success_response("user_memory", entries, "Entry added.")

    # -------------------------------------------------------------------------
    # Memory: Harvey observations
    # -------------------------------------------------------------------------

    def add_harvey_memory(self, entry: str) -> dict:
        """Add an entry about Harvey's observations (tool quirks, project facts).

        Entry format: one paragraph, auto-delimited with §.
        Writes to: data/Brain/pages/harvey_memory.md
        Character limit: 2200 chars.
        """
        entry = entry.strip()
        if not entry:
            return {"success": False, "error": "Entry cannot be empty."}

        scan_error = _scan_for_threats(entry)
        if scan_error:
            return {"success": False, "error": scan_error}

        path = PAGES_DIR / "harvey_memory.md"

        with _file_lock(path):
            entries = _read_memory_entries(path)

            if entry in entries:
                return self._success_response(
                    "harvey_memory", entries,
                    "Entry already exists (no duplicate added)."
                )

            new_entries = entries + [entry]
            new_total = len(ENTRY_DELIMITER.join(new_entries))

            if new_total > HARVEY_CHAR_LIMIT:
                current = len(ENTRY_DELIMITER.join(entries)) if entries else 0
                return {
                    "success": False,
                    "error": (
                        f"harvey_memory at {current:,}/{HARVEY_CHAR_LIMIT:,} chars. "
                        f"Adding this entry ({len(entry)} chars) would exceed the limit. "
                        f"Replace or remove existing entries first."
                    ),
                    "current_entries": entries,
                    "usage": f"{current:,}/{HARVEY_CHAR_LIMIT:,}",
                }

            entries.append(entry)
            _write_memory_entries(path, entries)

        return self._success_response("harvey_memory", entries, "Entry added.")

    # -------------------------------------------------------------------------
    # Memory: Replace
    # -------------------------------------------------------------------------

    def replace_memory(self, target: str, old_substring: str, new_content: str) -> dict:
        """Replace a memory entry by unique substring match.

        target: 'user_memory' or 'harvey_memory'
        """
        old_substring = old_substring.strip()
        new_content = new_content.strip()
        if not old_substring:
            return {"success": False, "error": "old_substring cannot be empty."}
        if not new_content:
            return {
                "success": False,
                "error": "new_content cannot be empty. Use 'remove' to delete."
            }

        scan_error = _scan_for_threats(new_content)
        if scan_error:
            return {"success": False, "error": scan_error}

        path = self._path_for_target(target)
        if path is None:
            return {
                "success": False,
                "error": f"Unknown target '{target}'. Use 'user_memory' or 'harvey_memory'."
            }

        limit = HARVEY_CHAR_LIMIT if target == "harvey_memory" else USER_CHAR_LIMIT

        with _file_lock(path):
            entries = _read_memory_entries(path)
            matches = [(i, e) for i, e in enumerate(entries) if old_substring in e]

            if len(matches) == 0:
                return {"success": False, "error": f"No entry matched '{old_substring}'."}

            if len(matches) > 1:
                unique_texts = set(e for _, e in matches)
                if len(unique_texts) > 1:
                    previews = [e[:80] + ("..." if len(e) > 80 else "") for _, e in matches]
                    return {
                        "success": False,
                        "error": f"Multiple entries matched '{old_substring}'. Be more specific.",
                        "matches": previews,
                    }

            idx = matches[0][0]

            test_entries = entries.copy()
            test_entries[idx] = new_content
            new_total = len(ENTRY_DELIMITER.join(test_entries))

            if new_total > limit:
                return {
                    "success": False,
                    "error": (
                        f"Replacement would put memory at {new_total:,}/{limit:,} chars. "
                        f"Shorten the new content or remove other entries first."
                    ),
                }

            entries[idx] = new_content
            _write_memory_entries(path, entries)

        return self._success_response(target, entries, "Entry replaced.")

    # -------------------------------------------------------------------------
    # Memory: Remove
    # -------------------------------------------------------------------------

    def remove_memory(self, target: str, substring: str) -> dict:
        """Remove a memory entry by unique substring match."""
        substring = substring.strip()
        if not substring:
            return {"success": False, "error": "substring cannot be empty."}

        path = self._path_for_target(target)
        if path is None:
            return {"success": False, "error": f"Unknown target '{target}'."}

        with _file_lock(path):
            entries = _read_memory_entries(path)
            matches = [(i, e) for i, e in enumerate(entries) if substring in e]

            if len(matches) == 0:
                return {"success": False, "error": f"No entry matched '{substring}'."}

            if len(matches) > 1:
                unique_texts = set(e for _, e in matches)
                if len(unique_texts) > 1:
                    previews = [e[:80] + ("..." if len(e) > 80 else "") for _, e in matches]
                    return {
                        "success": False,
                        "error": f"Multiple entries matched '{substring}'. Be more specific.",
                        "matches": previews,
                    }

            idx = matches[0][0]
            entries.pop(idx)
            _write_memory_entries(path, entries)

        return self._success_response(target, entries, "Entry removed.")

    # -------------------------------------------------------------------------
    # Skills: Create
    # -------------------------------------------------------------------------

    def create_skill(self, name: str, category: str, content: str) -> dict:
        """Create a new skill SKILL.md.

        content: full SKILL.md text including YAML frontmatter.
        Writes to: harvey-os/skills/<category>/<name>/SKILL.md

        Validates:
        - Skill name format (lowercase, max 64 chars, allowed chars)
        - Required frontmatter fields (name, description)
        - Threat scan on entire content
        - Path traversal protection
        """
        name = (name or "").strip().lower()
        category = (category or "").strip().lower()
        content = content.strip()

        if not name:
            return {"success": False, "error": "Skill name is required."}
        if not category:
            return {"success": False, "error": "Skill category is required."}
        if not content:
            return {"success": False, "error": "Skill content is required."}

        name_error = _validate_skill_name(name)
        if name_error:
            return {"success": False, "error": name_error}

        # Path traversal protection
        if ".." in category or "/" in category or "\\" in category:
            return {"success": False, "error": "Category path traversal not allowed."}
        if ".." in name or "/" in name or "\\" in name:
            return {"success": False, "error": "Skill name path traversal not allowed."}

        scan_error = _scan_for_threats(content)
        if scan_error:
            return {"success": False, "error": scan_error}

        frontmatter = _parse_yaml_frontmatter(content)
        fm_error = _validate_skill_frontmatter(frontmatter)
        if fm_error:
            return {"success": False, "error": fm_error}

        if frontmatter.get("name") != name:
            return {
                "success": False,
                "error": f"Frontmatter name '{frontmatter.get('name')}' does not match skill name '{name}'."
            }

        skill_dir = SKILLS_DIR / category / name
        skill_path = skill_dir / "SKILL.md"

        if skill_path.exists():
            return {"success": False, "error": f"Skill '{category}/{name}' already exists."}

        skill_dir.mkdir(parents=True, exist_ok=True)

        with _file_lock(skill_path):
            try:
                _atomic_write(skill_path, content)
            except RuntimeError as e:
                return {"success": False, "error": str(e)}

        return {
            "success": True,
            "skill": f"{category}/{name}",
            "path": str(skill_path),
            "message": "Skill created.",
        }

    # -------------------------------------------------------------------------
    # Skills: Patch
    # -------------------------------------------------------------------------

    def patch_skill(self, name: str, old_string: str, new_string: str) -> dict:
        """Patch an existing skill via find-replace.

        old_string: unique substring identifying the text to replace.
        new_string: replacement text.
        """
        name = (name or "").strip()
        old_string = old_string.strip()
        new_string = new_string.strip()

        if not name:
            return {"success": False, "error": "Skill name is required."}
        if not old_string:
            return {"success": False, "error": "old_string cannot be empty."}
        if not new_string:
            return {
                "success": False,
                "error": "new_string cannot be empty. Use 'remove' to delete."
            }

        scan_error = _scan_for_threats(new_string)
        if scan_error:
            return {"success": False, "error": scan_error}

        skill_path = self._find_skill_path(name)
        if skill_path is None:
            return {"success": False, "error": f"Skill '{name}' not found."}

        with _file_lock(skill_path):
            raw = skill_path.read_text(encoding="utf-8")

            if old_string not in raw:
                return {"success": False, "error": f"old_string not found in skill content."}

            new_content = raw.replace(old_string, new_string, 1)

            frontmatter = _parse_yaml_frontmatter(new_content)
            fm_error = _validate_skill_frontmatter(frontmatter)
            if fm_error:
                return {
                    "success": False,
                    "error": f"Patch would break skill frontmatter: {fm_error}"
                }

            try:
                _atomic_write(skill_path, new_content)
            except RuntimeError as e:
                return {"success": False, "error": str(e)}

        return {
            "success": True,
            "skill": name,
            "path": str(skill_path),
            "message": "Skill patched.",
        }

    # -------------------------------------------------------------------------
    # Utility
    # -------------------------------------------------------------------------

    def _path_for_target(self, target: str) -> Optional[Path]:
        if target == "user_memory":
            return PAGES_DIR / "user_memory.md"
        elif target == "harvey_memory":
            return PAGES_DIR / "harvey_memory.md"
        return None

    def _find_skill_path(self, name: str) -> Optional[Path]:
        """Search skills dirs for a skill by name, return path or None."""
        name = name.strip().lower()
        for category_dir in SKILLS_DIR.iterdir():
            if not category_dir.is_dir():
                continue
            skill_path = category_dir / name / "SKILL.md"
            if skill_path.exists():
                return skill_path
        return None

    def _success_response(
        self, target: str, entries: List[str], message: str = None
    ) -> dict:
        limit = HARVEY_CHAR_LIMIT if target == "harvey_memory" else USER_CHAR_LIMIT
        current = len(ENTRY_DELIMITER.join(entries)) if entries else 0
        pct = int((current / limit) * 100) if limit > 0 else 0
        return {
            "success": True,
            "target": target,
            "entries": entries,
            "usage": f"{pct}% — {current:,}/{limit:,} chars",
            "entry_count": len(entries),
            "message": message,
        }
