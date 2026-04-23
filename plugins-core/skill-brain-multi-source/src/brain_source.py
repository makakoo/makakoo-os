"""BrainSource — adapter interface for pluggable knowledge substrates.

A BrainSource is anything Harvey treats as "a place knowledge lives": a Logseq
graph, an Obsidian vault, a plain-markdown directory, eventually a Notion export
or a cloud drive. Every source knows:

- where its files live
- how to name a daily journal
- how to format new content (outliner bullets vs flat prose)
- how to list all its notes for ingestion

The upstream `core.memory.brain_bridge` module is Logseq-specific and used in
~40 call sites. We don't touch it here. Instead we introduce the adapter
pattern alongside; new code uses adapters, legacy code keeps working. v0.2
sprint Phase C will migrate the legacy call sites in one pass.
"""

from __future__ import annotations

import os
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Iterable, Iterator


@dataclass
class BrainDoc:
    """One markdown file discovered in a brain source, normalized for ingest."""

    source_name: str
    relative_path: str  # relative to the source's root
    absolute_path: Path
    title: str          # derived from filename or first-H1
    body: str
    mtime: float


class BrainSource(ABC):
    """A pluggable knowledge substrate."""

    name: str
    root: Path
    writable: bool

    @abstractmethod
    def journal_path(self, d: date) -> Path:
        """Return the absolute path of the journal file for the given date."""

    @abstractmethod
    def format_journal_line(self, text: str) -> str:
        """Format a single line for this source's style (e.g. outliner vs flat)."""

    @abstractmethod
    def page_path(self, title: str) -> Path:
        """Return the path where a page with the given title would live."""

    def iter_docs(self) -> Iterator[BrainDoc]:
        """Walk the source and yield every markdown document."""
        if not self.root.exists():
            return
        for path in sorted(self.root.rglob("*.md")):
            # Skip hidden dirs like .obsidian/, .logseq/bak/
            if any(part.startswith(".") for part in path.relative_to(self.root).parts):
                continue
            try:
                body = path.read_text(encoding="utf-8", errors="replace")
                mtime = path.stat().st_mtime
            except OSError:
                continue
            title = self._derive_title(path, body)
            yield BrainDoc(
                source_name=self.name,
                relative_path=str(path.relative_to(self.root)),
                absolute_path=path,
                title=title,
                body=body,
                mtime=mtime,
            )

    def append_to_journal(self, text: str, d: date | None = None) -> Path:
        """Append a formatted line to today's (or the specified date's) journal.

        Creates the journal file if absent. Raises PermissionError if the source
        is read-only.
        """
        if not self.writable:
            raise PermissionError(f"BrainSource '{self.name}' is read-only")
        d = d or date.today()
        path = self.journal_path(d)
        path.parent.mkdir(parents=True, exist_ok=True)
        line = self.format_journal_line(text.rstrip())
        existing = path.read_text(encoding="utf-8") if path.exists() else ""
        sep = "" if existing.endswith("\n") or not existing else "\n"
        path.write_text(existing + sep + line + "\n", encoding="utf-8")
        return path

    @staticmethod
    def _derive_title(path: Path, body: str) -> str:
        """Title is first H1 if present, else filename stem with separators stripped."""
        m = re.search(r"^#\s+(.+?)\s*$", body, re.MULTILINE)
        if m:
            return m.group(1).strip()
        return path.stem.replace("_", " ").replace("-", " ").strip()


class LogseqSource(BrainSource):
    """Logseq-flavored brain: outliner bullets, journals named YYYY_MM_DD.md."""

    def __init__(self, name: str, root: Path, writable: bool = True):
        self.name = name
        self.root = root
        self.writable = writable

    def journal_path(self, d: date) -> Path:
        return self.root / "journals" / f"{d.strftime('%Y_%m_%d')}.md"

    def format_journal_line(self, text: str) -> str:
        # Every line in a Logseq outliner starts with "- "
        # If the caller already passed a "- " prefix, respect it
        if text.lstrip().startswith("- "):
            return text
        return f"- {text}"

    def page_path(self, title: str) -> Path:
        safe = re.sub(r"[^a-zA-Z0-9 _\-.]", "_", title)
        return self.root / "pages" / f"{safe}.md"


class ObsidianSource(BrainSource):
    """Obsidian vault: flat markdown, daily notes named per vault config.

    Respects `.obsidian/daily-notes.json` if present. Defaults to `YYYY-MM-DD.md`
    under the vault root if no config is found.
    """

    def __init__(self, name: str, root: Path, writable: bool = True):
        self.name = name
        self.root = root
        self.writable = writable
        self._daily_format, self._daily_folder = self._read_daily_config()

    def _read_daily_config(self) -> tuple[str, str]:
        cfg = self.root / ".obsidian" / "daily-notes.json"
        if cfg.exists():
            try:
                import json
                data = json.loads(cfg.read_text(encoding="utf-8"))
                fmt = data.get("format") or "YYYY-MM-DD"
                folder = data.get("folder") or ""
                return fmt, folder
            except (json.JSONDecodeError, OSError):
                pass
        return "YYYY-MM-DD", ""

    def _strftime(self, d: date) -> str:
        # Translate Obsidian's Moment.js format tokens (YYYY/MM/DD) to strftime
        translation = {
            "YYYY": "%Y", "YY": "%y",
            "MM": "%m", "M": str(d.month),
            "DD": "%d", "D": str(d.day),
        }
        out = self._daily_format
        # Replace longest tokens first to avoid YY eating YYYY
        for token in sorted(translation.keys(), key=len, reverse=True):
            out = out.replace(token, translation[token])
        return d.strftime(out) if "%" in out else out

    def journal_path(self, d: date) -> Path:
        folder = self.root / self._daily_folder if self._daily_folder else self.root
        return folder / f"{self._strftime(d)}.md"

    def format_journal_line(self, text: str) -> str:
        # Obsidian vaults use flat prose by default
        return text

    def page_path(self, title: str) -> Path:
        safe = re.sub(r"[^a-zA-Z0-9 _\-.]", "_", title)
        return self.root / f"{safe}.md"


class PlainMarkdownSource(BrainSource):
    """Fallback: any directory of markdown files, daily notes at root."""

    def __init__(self, name: str, root: Path, writable: bool = True,
                 daily_format: str = "%Y-%m-%d", write_style: str = "flat"):
        self.name = name
        self.root = root
        self.writable = writable
        self.daily_format = daily_format
        self.write_style = write_style

    def journal_path(self, d: date) -> Path:
        return self.root / f"{d.strftime(self.daily_format)}.md"

    def format_journal_line(self, text: str) -> str:
        if self.write_style == "outliner" and not text.lstrip().startswith("- "):
            return f"- {text}"
        return text

    def page_path(self, title: str) -> Path:
        safe = re.sub(r"[^a-zA-Z0-9 _\-.]", "_", title)
        return self.root / f"{safe}.md"


def build_source(entry: dict) -> BrainSource:
    """Factory: build a BrainSource from a brain_sources.json entry."""
    stype = entry.get("type", "logseq")
    name = entry.get("name", stype)
    path_str = os.path.expandvars(os.path.expanduser(entry.get("path", "")))
    root = Path(path_str)
    writable = bool(entry.get("writable", True))

    if stype == "logseq":
        return LogseqSource(name=name, root=root, writable=writable)
    if stype == "obsidian":
        return ObsidianSource(name=name, root=root, writable=writable)
    if stype == "plain":
        return PlainMarkdownSource(
            name=name,
            root=root,
            writable=writable,
            daily_format=entry.get("daily_format", "%Y-%m-%d"),
            write_style=entry.get("write_style", "flat"),
        )
    raise ValueError(f"unknown brain source type: {stype!r}")
