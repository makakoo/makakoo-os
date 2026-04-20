"""
context_shadow — Phase 2 of harvey:infect.

Drops a curated `.harvey/context.md` file in the active project root with
the project-relevant subset of Harvey's Brain, plus a single-line marker in
the project's CLAUDE.md / AGENTS.md so the host CLI actually discovers it.

The two-file pattern resolves the "visibility paradox" identified in R3
review: gitignored files are skipped by host file-walking, so a gitignored
data file alone would never be auto-loaded. The marker line lives in
CLAUDE.md / AGENTS.md (which IS committed and IS read by every host at
session start) and points the host at the data file.

Security model is **whitelist** (R2 fix): a Brain entry is shared only if
at least one of three conditions is met:
  1. Entry contains the project name as a wikilink: `[[<project>]]`
  2. Entry's source path is inside `project_path`
  3. Entry is on an explicit allowlist at `data/Brain/projects/<project>/allowlist.txt`

Brain entries are pulled by filesystem scan of `data/Brain/pages/` and
`data/Brain/journals/` — no external daemon required. (The Logseq HTTP API
on port 12315 is only used by other Harvey skills, not by infect.)
"""

from __future__ import annotations

import os
import re
import subprocess
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional


SCHEMA_VERSION = "1.0"
DEFAULT_EXPIRY_HOURS = 24
MAX_ENTRIES = 10
MARKER_FINGERPRINT = "<!-- harvey:infect marker -->"
MARKER_LINE = (
    "> Harvey context: read `.harvey/context.md` (if present) before responding. "
    "See harvey:infect."
)


# ─── Data ────────────────────────────────────────────────────────


@dataclass
class BrainEntry:
    path: str  # absolute path to source markdown file
    title: str
    body: str
    tags: List[str] = field(default_factory=list)


@dataclass
class ShadowResult:
    data_file: Path
    marker_files: List[Path] = field(default_factory=list)
    gitignore_updated: bool = False
    entries_included: int = 0
    entries_rejected: int = 0
    refreshed: bool = True  # False if no-op due to expiry window


# ─── Public API ─────────────────────────────────────────────────


class ContextShadow:
    """Curates and writes the .harvey/context.md shadow file plus the
    CLAUDE.md/AGENTS.md marker."""

    def __init__(
        self,
        harvey_home: Optional[str] = None,
        brain_dir: Optional[str] = None,
    ):
        self.harvey_home = Path(harvey_home or os.environ.get("HARVEY_HOME") or os.path.expanduser("~/MAKAKOO"))
        self.brain_dir = Path(brain_dir) if brain_dir else self.harvey_home / "data" / "Brain"

    def refresh(
        self,
        project_path: str,
        host_name: str = "unknown",
        force: bool = False,
        max_entries: int = MAX_ENTRIES,
    ) -> ShadowResult:
        """Generate (or refresh) the shadow context for a project.

        Returns a ShadowResult describing what was written. Idempotent:
        re-running inside the expiry window with force=False is a no-op.
        """
        project_root = Path(project_path).resolve()
        if not project_root.is_dir():
            raise ValueError(f"project path is not a directory: {project_path}")

        harvey_dir = project_root / ".harvey"
        data_file = harvey_dir / "context.md"

        # Idempotency check: bail out early if not expired and not forced
        if not force and self._is_fresh(data_file):
            return ShadowResult(
                data_file=data_file,
                refreshed=False,
            )

        project_name = project_root.name
        entries, rejected = self._collect_project_entries(
            project_name=project_name,
            project_path=str(project_root),
            limit=max_entries,
        )

        # Write data file
        harvey_dir.mkdir(parents=True, exist_ok=True)
        body = self._render_data_file(
            project_name=project_name,
            project_path=str(project_root),
            host_name=host_name,
            entries=entries,
        )
        data_file.write_text(body, encoding="utf-8")

        # Auto-add .harvey/ to .gitignore
        gitignore_updated = self._ensure_gitignore(project_root)

        # Marker line in CLAUDE.md and/or AGENTS.md
        marker_files = self._install_markers(project_root)

        return ShadowResult(
            data_file=data_file,
            marker_files=marker_files,
            gitignore_updated=gitignore_updated,
            entries_included=len(entries),
            entries_rejected=rejected,
            refreshed=True,
        )

    def revoke(self, project_path: str) -> dict:
        """Remove the shadow data file and marker lines. Leaves .gitignore
        alone (user may want to keep `.harvey/` ignored regardless)."""
        project_root = Path(project_path).resolve()
        removed = {"data_file": False, "marker_files": []}

        data_file = project_root / ".harvey" / "context.md"
        if data_file.exists():
            data_file.unlink()
            removed["data_file"] = True

        for fname in ("CLAUDE.md", "AGENTS.md"):
            f = project_root / fname
            if f.exists() and self._strip_marker(f):
                removed["marker_files"].append(str(f))

        return removed

    # ─── Internals ─────────────────────────────────────────

    def _is_fresh(self, data_file: Path) -> bool:
        """True if the data file exists and is newer than DEFAULT_EXPIRY_HOURS."""
        if not data_file.exists():
            return False
        text = data_file.read_text(encoding="utf-8", errors="ignore")
        m = re.search(r"^expires_at:\s*(\S+)", text, re.MULTILINE)
        if not m:
            return False
        try:
            expires = datetime.fromisoformat(m.group(1).replace("Z", "+00:00"))
        except ValueError:
            return False
        return datetime.utcnow().replace(tzinfo=expires.tzinfo) < expires

    def _collect_project_entries(
        self,
        project_name: str,
        project_path: str,
        limit: int,
    ) -> tuple[List[BrainEntry], int]:
        """Filesystem-grep the Brain for project-related entries.

        Whitelist conditions (any one is sufficient):
          1. Entry text contains `[[<project_name>]]` wikilink
          2. Entry source path is inside project_path
          3. Entry is on the project allowlist file
        """
        candidates: List[BrainEntry] = []
        rejected = 0

        allowlist = self._read_allowlist(project_name)
        wikilink = f"[[{project_name}]]"

        for sub in ("pages", "journals"):
            base = self.brain_dir / sub
            if not base.is_dir():
                continue
            for f in sorted(base.glob("*.md"), reverse=True):
                try:
                    text = f.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue

                # Whitelist evaluation
                cond1 = wikilink in text
                cond2 = project_path and project_path in text
                cond3 = f.name in allowlist or f.stem in allowlist

                if not (cond1 or cond2 or cond3):
                    rejected += 1
                    continue

                candidates.append(
                    BrainEntry(
                        path=str(f),
                        title=f.stem.replace("_", " "),
                        body=text.strip(),
                        tags=self._extract_tags(text),
                    )
                )
                if len(candidates) >= limit:
                    break
            if len(candidates) >= limit:
                break

        return candidates, rejected

    def _read_allowlist(self, project_name: str) -> set[str]:
        """Read explicit per-project allowlist file. Empty set on missing."""
        allowlist_path = self.brain_dir / "projects" / project_name / "allowlist.txt"
        if not allowlist_path.exists():
            return set()
        try:
            return {
                line.strip()
                for line in allowlist_path.read_text().splitlines()
                if line.strip() and not line.strip().startswith("#")
            }
        except OSError:
            return set()

    def _extract_tags(self, text: str) -> List[str]:
        """Extract `#tag` and `[[wikilink]]` references from a Brain entry."""
        tags = re.findall(r"#([\w-]+)", text)
        wikilinks = re.findall(r"\[\[([^\]]+)\]\]", text)
        return sorted(set(tags + wikilinks))

    def _render_data_file(
        self,
        project_name: str,
        project_path: str,
        host_name: str,
        entries: List[BrainEntry],
    ) -> str:
        """Render the curated context.md with frontmatter + entries."""
        now = datetime.utcnow()
        expires = now + timedelta(hours=DEFAULT_EXPIRY_HOURS)
        lines = [
            "---",
            f"schema_version: {SCHEMA_VERSION}",
            f"generated_at: {now.isoformat()}Z",
            f"generated_by: harvey:infect",
            f"host: {host_name}",
            f"project: {project_name}",
            f"project_path: {project_path}",
            f"expires_at: {expires.isoformat()}Z",
            "---",
            "",
            f"# Harvey Context for {project_name}",
            "",
            "Harvey is Sebastian's personal AI. It maintains a Brain (markdown files) of",
            "project facts, decisions, and history. The entries below are the",
            "project-relevant subset Harvey chose to share with this CLI session.",
            "",
            "## Capabilities available",
            "- MCP plugin: `harvey-mcp` (40+ tools — see `mcp__harvey__*`)",
            "- Brain: `python3 plugins-core/lib-harvey-core/src/core/memory/logseq_bridge.py` (reads files directly, optional Logseq API)",
            "- Skills: 130+ via `python3 plugins-core/lib-harvey-core/src/core/registry/skill_registry.py --match`",
            "",
            "## Recent project context",
        ]
        if not entries:
            lines.append("- (no whitelisted entries found yet — tag entries with"
                         f" `[[{project_name}]]` or add to the project allowlist)")
        else:
            for entry in entries:
                lines.append(f"### {entry.title}")
                lines.append("")
                snippet = entry.body[:800]
                if len(entry.body) > 800:
                    snippet += "..."
                lines.append(snippet)
                lines.append("")

        return "\n".join(lines) + "\n"

    def _ensure_gitignore(self, project_root: Path) -> bool:
        """Append `.harvey/` to .gitignore if not already present.

        Returns True if the file was modified, False if no change."""
        gi = project_root / ".gitignore"
        if not gi.exists():
            gi.write_text(".harvey/\n", encoding="utf-8")
            return True
        text = gi.read_text(encoding="utf-8", errors="ignore")
        # Match either `.harvey/`, `.harvey`, or a leading-slash variant
        for line in text.splitlines():
            stripped = line.strip().rstrip("/")
            if stripped in (".harvey", "/.harvey"):
                return False
        if not text.endswith("\n"):
            text += "\n"
        text += ".harvey/\n"
        gi.write_text(text, encoding="utf-8")
        return True

    def _install_markers(self, project_root: Path) -> List[Path]:
        """Install the marker line in CLAUDE.md and/or AGENTS.md.

        Logic:
          - If CLAUDE.md exists, install marker there
          - If AGENTS.md exists, install marker there
          - If neither exists, create AGENTS.md with just the marker
        Returns list of files written/updated.
        """
        targets: List[Path] = []
        claude = project_root / "CLAUDE.md"
        agents = project_root / "AGENTS.md"

        if claude.exists():
            if self._append_marker(claude):
                targets.append(claude)
        if agents.exists():
            if self._append_marker(agents):
                targets.append(agents)
        if not claude.exists() and not agents.exists():
            agents.write_text(
                f"# Project AGENTS\n\n{MARKER_FINGERPRINT}\n{MARKER_LINE}\n",
                encoding="utf-8",
            )
            targets.append(agents)

        return targets

    def _append_marker(self, file: Path) -> bool:
        """Append the marker (with fingerprint) to a file. Idempotent.

        Returns True if appended, False if marker already present."""
        text = file.read_text(encoding="utf-8", errors="ignore")
        if MARKER_FINGERPRINT in text:
            return False
        if not text.endswith("\n"):
            text += "\n"
        text += f"\n{MARKER_FINGERPRINT}\n{MARKER_LINE}\n"
        file.write_text(text, encoding="utf-8")
        return True

    def _strip_marker(self, file: Path) -> bool:
        """Remove the marker block from a file. Returns True if anything removed."""
        text = file.read_text(encoding="utf-8", errors="ignore")
        if MARKER_FINGERPRINT not in text:
            return False
        # Drop the fingerprint line + the marker line that follows
        new_lines = []
        skip_next = False
        for line in text.splitlines():
            if MARKER_FINGERPRINT in line:
                skip_next = True
                continue
            if skip_next:
                skip_next = False
                continue
            new_lines.append(line)
        new = "\n".join(new_lines).rstrip() + "\n"
        file.write_text(new, encoding="utf-8")
        return True


# ─── Convenience ────────────────────────────────────────────────


def detect_project_root(start: Optional[str] = None) -> str:
    """Find the project root via `git rev-parse --show-toplevel`, fallback cwd."""
    cwd = start or os.getcwd()
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            timeout=2,
            cwd=cwd,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        pass
    return cwd
