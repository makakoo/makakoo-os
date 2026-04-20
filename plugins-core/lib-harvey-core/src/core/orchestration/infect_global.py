"""
⚠️ DEPRECATED — Use `makakoo infect --global` (Rust) instead.

The Rust implementation at makakoo-os/makakoo/src/infect/ is now authoritative.
It supports dynamic bootstrap-fragment assembly from plugins, host-scoped
fragments, and caching. This Python version is kept for backward compatibility
only. No new features will be added here.

---
infect_global — Global Harvey parasite across all CLI hosts.

Per-project `infect.py` drops `.harvey/context.md` + marker into a single
project. This module is the **second strain** of the parasite: it writes a
canonical Harvey Bootstrap Block into each CLI's *global* config slot, so
Harvey identity + caveman voice + brain access + superbrain pointer fire
from any directory, not just from inside an infected project.

Slots (verified 2026-04-12 against the live machine):

  | Host         | Global slot path                          | Format          |
  |--------------|-------------------------------------------|-----------------|
  | claude-code  | ~/.claude/CLAUDE.md                       | markdown        |
  | gemini-cli   | ~/.gemini/GEMINI.md                       | markdown        |
  | codex        | ~/.codex/instructions.md                  | markdown        |
  | opencode     | ~/.config/opencode/opencode.json          | opencode_json   |

Markdown slots get a fenced bootstrap block appended (or updated) at the
end of the file, identified by idempotent marker comments:

    <!-- harvey:infect-global START vN -->
    ...bootstrap content...
    <!-- harvey:infect-global END -->

OpenCode uses JSON, so the same bootstrap content goes into the config's
`instructions` array as a single string tagged with a stable prefix
("[harvey:infect-global v1] ..."). Every other JSON key is preserved
byte-for-byte — the entire JSON object is re-serialized with indent=2.

Idempotency: re-running `install_all()` at the same version is a no-op
(returns UNCHANGED). Re-running after a version bump replaces the block
in place. `revoke_all()` strips the block cleanly and leaves every other
line / JSON key alone.

Atomic writes: every filesystem mutation goes through a temp file in the
same directory, then `os.replace()` to swap it into place. No partial
writes on crash. Parent directories are created when missing.

LIMITATIONS (per lope R1 review, 2026-04-12):
  - Only ONE global block per markdown file is supported. If a user
    manually pastes a second `<!-- harvey:infect-global START -->` pair
    into the same file, `_BLOCK_RE` will match the first pair and the
    second becomes orphaned content. install/revoke operate on the
    first match only.
  - `BOOTSTRAP_BODY` is a module-level string constant, so edits to
    the canonical rules in ~/MAKAKOO/CLAUDE.md do not propagate to
    already-infected hosts until someone re-runs `--global`. The
    inline block is a cache; the SSoT remains the pointer to
    ~/MAKAKOO/CLAUDE.md which the agent is instructed to read on session
    start. Bump BLOCK_VERSION to force a fleet-wide in-place upgrade.
"""

from __future__ import annotations

import json
import os
import re
import tempfile
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Dict, List, Literal, Optional, Tuple

from .host_detector import HostType


# ─── Bootstrap block version + content ─────────────────────────
#
# Bump BLOCK_VERSION any time the bootstrap content materially changes.
# install_all() will then replace the existing block on every machine
# on the next run. Never reuse an old version number.
#
# The fingerprint regex is version-agnostic so we can detect and replace
# stale blocks from any earlier version.

BLOCK_VERSION = "10"
BLOCK_FINGERPRINT = "harvey:infect-global"
BLOCK_START = f"<!-- harvey:infect-global START v{BLOCK_VERSION} -->"
BLOCK_END = "<!-- harvey:infect-global END -->"
# v5 (2026-04-14): platform rename Harvey OS → Makakoo OS. Persona stays "Harvey";
# MCP server name, tool prefixes, and runtime subdir are unchanged.
# v6 (2026-04-14): added omni multimodal block — every CLI now sees
# harvey_describe_image/audio/video MCP tools + the core.llm.omni Python
# API for image/audio/video understanding via xiaomi-tp:mimo-v2-omni.
# v7 (2026-04-14): Qwen Code joins the family as the 7th CLI host.
# New GlobalSlot at ~/.qwen/QWEN.md, HostType.QWEN in host_detector,
# Qwen added to the lope validator team. Bootstrap body updated to
# list all 7 hosts.
# v8 (2026-04-14): closed the qwen auto-memory sync gap.
# v9 (2026-04-16): pi coding agent joins as the 9th CLI host.
# New GlobalSlot at ~/.pi/AGENTS.md, HostType.PI in host_detector.
# Bootstrap body updated to list all 8 hosts including pi.
# v10 (2026-04-20): describe-vs-ingest dichotomy + rate-limit rule.
# Codifies harvey_knowledge_ingest as the "add to knowledge" path and
# describe_* as one-shot Q&A. Bans the URL-in-journal confabulation
# caught in opencode 2026-04-20 on youtu.be/fdbXNWkpPMY.
# v8 (2026-04-14): closed the qwen auto-memory sync gap — added
# ~/.qwen/memory to AUTO_MEMORY_SYMLINKS and updated the bootstrap
# body to list all 7 memory symlink paths (was stale at 6).
# The marker string stays `harvey:infect-global` so the version-agnostic
# regex below still matches prior v1-v7 blocks for in-place upgrade.

# Version-agnostic regex — catches ANY prior version for replacement.
_BLOCK_RE = re.compile(
    r"\n*<!--\s*harvey:infect-global\s+START\s+v[^\s>]+\s*-->.*?<!--\s*harvey:infect-global\s+END\s*-->\n*",
    re.DOTALL,
)

# OpenCode JSON tag — single string in the `instructions` array.
JSON_TAG_PREFIX = f"[harvey:infect-global v{BLOCK_VERSION}]"
_JSON_TAG_FINGERPRINT = "[harvey:infect-global"

# Canonical bootstrap source lives at $MAKAKOO_HOME/harvey-os/global_bootstrap.md.
# v3 onward loads from this file so edits don't require touching Python.
# Fallback to inline string if the file is missing (e.g. running tests
# outside the repo, or after `harvey-os/` is renamed — defensive).
_CANONICAL_BOOTSTRAP_PATH = (
    Path(__file__).resolve().parent.parent.parent / "global_bootstrap.md"
)


def _resolved_home_or_placeholder() -> str:
    """Best-effort platform home for the fallback bootstrap. Never raises.

    v5 (2026-04-14): reads MAKAKOO_HOME first, falls back to HARVEY_HOME for
    legacy compat, then to the literal `$MAKAKOO_HOME` placeholder if neither
    is set. The placeholder is intentional — it keeps the fallback string
    path-agnostic so it works on any install.
    """
    for env_var in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(env_var)
        if val:
            return os.path.abspath(os.path.expanduser(val))
    return "$MAKAKOO_HOME"


_FALLBACK_BOOTSTRAP_BODY = """# Makakoo OS — Global Bootstrap (v{version})

You are **Harvey**, Sebastian Schkudlara's autonomous cognitive extension. The platform is **Makakoo OS** (renamed from Harvey OS 2026-04-14); the persona is still Harvey. Canonical bootstrap file was missing at load time; this is a minimal fallback. Full operating manual at `{home}/CLAUDE.md`. Auto-memory index at `{home}/data/auto-memory/MEMORY.md`.

- Zero sycophancy. Radical competence. Sharp, concise tone.
- Never auto-send emails or LinkedIn messages — draft first.
- Log significant work to `{home}/data/Brain/journals/YYYY_MM_DD.md`.
- Query `superbrain search "topic"` before claiming ignorance.
- Read `{home}/data/auto-memory/MEMORY.md` at session start.
"""


def _load_bootstrap_body() -> str:
    """Load the canonical bootstrap content from the source-of-truth file.

    Falls back to an inline minimal string if the file is missing so the
    infector still works outside a full Harvey checkout (e.g. tests).
    """
    if _CANONICAL_BOOTSTRAP_PATH.is_file():
        return _CANONICAL_BOOTSTRAP_PATH.read_text(encoding="utf-8").rstrip() + "\n"
    return _FALLBACK_BOOTSTRAP_BODY.format(
        version=BLOCK_VERSION,
        home=_resolved_home_or_placeholder(),
    )


BOOTSTRAP_BODY = _load_bootstrap_body()


def _render_markdown_block() -> str:
    """Produce the full fenced block, including start/end markers."""
    # v3: BOOTSTRAP_BODY is loaded from global_bootstrap.md at import time.
    # The file uses `{version}` literal in content rarely; .format would
    # break on curly braces so we use .replace for version substitution.
    body = BOOTSTRAP_BODY.replace("{version}", BLOCK_VERSION)
    return f"{BLOCK_START}\n{body}\n{BLOCK_END}\n"


def _render_opencode_instruction() -> str:
    """Produce the single JSON-array-entry string for opencode."""
    body = BOOTSTRAP_BODY.replace("{version}", BLOCK_VERSION)
    return f"{JSON_TAG_PREFIX} {body}"


# ─── Data ──────────────────────────────────────────────────────


class SlotStatus(Enum):
    INSTALLED = "installed"          # block did not exist before
    UPDATED = "updated"              # prior version replaced in place
    UNCHANGED = "unchanged"          # same version already present
    REVOKED = "revoked"              # block removed (revoke mode)
    ABSENT = "absent"                # nothing to revoke (revoke mode)
    SKIPPED = "skipped"              # parent dir missing + --skip-missing
    DRY_RUN = "dry-run"              # would write, but --dry-run
    ERROR = "error"                  # write failed or invalid existing state


SlotFormat = Literal["markdown", "opencode_json"]


@dataclass(frozen=True)
class GlobalSlot:
    host: HostType
    rel_path: str        # path relative to $HOME, POSIX-style
    format: SlotFormat
    display_name: str = ""  # v8: actual name when host == DYNAMIC (runtime-registered)

    @property
    def label(self) -> str:
        """Human-readable label for this slot — falls back to host.value."""
        return self.display_name or self.host.value


@dataclass
class GlobalInfectResult:
    host: HostType
    path: Path
    status: SlotStatus
    version: str = ""                # version currently in file
    previous_version: str = ""       # version that was there before
    error: str = ""
    display_name: str = ""           # v8: echoed from the slot for dynamic hosts

    @property
    def label(self) -> str:
        """Human-readable label — used by CLI output. Dynamic hosts store
        their actual name in `display_name`; built-in hosts fall through
        to `host.value`.
        """
        return self.display_name or self.host.value


# v3: audit / verify reports
#
# AuditReport captures the pre-block content of every slot plus any
# stale-fact drifts detected. Pre-block content is whatever lives BEFORE
# the harvey infect markers — Gemini's legacy `## Gemini Added Memories`
# section is the most common source of stale facts that silently override
# the fresh bootstrap.


@dataclass
class SlotSnapshot:
    host: HostType
    path: Path
    pre_block: str          # content before the harvey block
    block_version: str      # version tag of the current block, or ""
    pre_block_bytes: int


@dataclass
class DriftFinding:
    host: HostType
    path: Path
    pattern_label: str      # human-readable label for the stale fact
    excerpt: str            # up to 200 chars around the match


@dataclass
class AuditReport:
    snapshots: List[SlotSnapshot]
    drifts: List[DriftFinding]

    def has_drift(self) -> bool:
        return bool(self.drifts)


@dataclass
class SymlinkStatus:
    path: Path
    target: Optional[Path]
    ok: bool
    reason: str = ""


@dataclass
class VerifyReport:
    slots: List[GlobalInfectResult]
    symlinks: List[SymlinkStatus]
    canonical_memory_exists: bool

    def is_clean(self) -> bool:
        slots_ok = all(s.status == SlotStatus.UNCHANGED for s in self.slots
                       if s.status not in (SlotStatus.SKIPPED, SlotStatus.ABSENT))
        symlinks_ok = all(s.ok for s in self.symlinks)
        return slots_ok and symlinks_ok and self.canonical_memory_exists


# Stale-fact patterns (known Gemini drift + legacy paths).
# Each entry: (compiled regex, human-readable label).
# Extend this list as new drifts are discovered.
_STALE_PATTERNS: List[Tuple[re.Pattern, str]] = [
    (re.compile(r"Sebastian_Schkudlara_CV_2025", re.IGNORECASE),
     "stale CV year (2025; current is 2026)"),
    (re.compile(r"/HARVEY/career/SKILL\.md", re.IGNORECASE),
     "legacy career-agent path (now at /HARVEY/agents/career-manager)"),
    (re.compile(r"/HARVEY/skills/career-manager", re.IGNORECASE),
     "legacy career-manager skill path (moved to agents/career-manager)"),
    (re.compile(r"/HARVEY/videocr/", re.IGNORECASE),
     "legacy videocr path (moved under skills/video-ocr)"),
    (re.compile(r"/HARVEY/Brain\b", re.IGNORECASE),
     "legacy Brain path (/HARVEY/Brain/; current is /HARVEY/data/Brain/)"),
    (re.compile(r"large language model,?\s+trained by Google", re.IGNORECASE),
     "Gemini default identity override (conflicts with Harvey identity)"),
    (re.compile(r"I am a (?:large )?language model\b", re.IGNORECASE),
     "LLM default identity override (conflicts with Harvey identity)"),
    (re.compile(r"sprint-forge", re.IGNORECASE),
     "legacy lope project name (renamed 2026-04-10)"),
]


def _split_markdown_pre_block(text: str) -> Tuple[str, str]:
    """Return (content_before_block, block_version) for a markdown slot.

    If no block is present, the entire file content is pre-block and
    version is "". If a block is present, content BEFORE the START marker
    is the pre-block.
    """
    found = _find_markdown_block(text)
    if found is None:
        return (text, "")
    start, _end, version = found
    return (text[:start], version)


def _describe_opencode_non_harvey(data: dict) -> Tuple[str, str]:
    """Summarize the non-harvey portion of opencode.json for audit.

    Returns (summary_string, harvey_entry_version). Non-harvey content
    includes: top-level keys other than `instructions`, and any entries
    in the `instructions` array that are NOT tagged as harvey:infect-global.
    """
    summary_parts: List[str] = []
    for key in sorted(data.keys()):
        if key == "instructions":
            continue
        summary_parts.append(f"{key}: {type(data[key]).__name__}")

    harvey_version = ""
    non_harvey_instructions: List[str] = []
    for entry in data.get("instructions", []) or []:
        if isinstance(entry, str) and _JSON_TAG_FINGERPRINT in entry[:40]:
            harvey_version = _opencode_entry_version(entry)
        else:
            preview = str(entry)[:80]
            non_harvey_instructions.append(preview)

    if non_harvey_instructions:
        summary_parts.append(f"non-harvey instructions: {len(non_harvey_instructions)} entries")
        summary_parts.extend(f"  {p}" for p in non_harvey_instructions)

    return ("\n".join(summary_parts), harvey_version)


def _excerpt_for(text: str, pattern: re.Pattern, maxlen: int = 200) -> str:
    """Return a short excerpt of `text` around the first match of `pattern`."""
    m = pattern.search(text)
    if m is None:
        return ""
    start = max(0, m.start() - 40)
    end = min(len(text), m.end() + 40)
    excerpt = text[start:end].replace("\n", " ").strip()
    if len(excerpt) > maxlen:
        excerpt = excerpt[:maxlen] + "..."
    return excerpt


# Canonical slot table — single source of truth.
# v3 adds VIBE (Mistral Vibe) and CURSOR. Cursor's global rules file is
# opt-in — users must enable "Rules for AI" in Cursor settings before the
# file has any effect — but we still write it so the content is ready.
SLOTS: List[GlobalSlot] = [
    GlobalSlot(HostType.CLAUDE_CODE, ".claude/CLAUDE.md", "markdown"),
    GlobalSlot(HostType.GEMINI_CLI, ".gemini/GEMINI.md", "markdown"),
    GlobalSlot(HostType.CODEX, ".codex/instructions.md", "markdown"),
    GlobalSlot(HostType.OPENCODE, ".config/opencode/opencode.json", "opencode_json"),
    GlobalSlot(HostType.VIBE, ".vibe/instructions.md", "markdown"),
    GlobalSlot(HostType.CURSOR, ".cursor/rules.md", "markdown"),
    # Qwen Code — added 2026-04-14 as the 7th host. Gemini-CLI fork at
    # github.com/QwenLM/qwen-code. User-level context file is QWEN.md
    # (confirmed via the bundled locale strings — "Analyzes the project
    # and creates a tailored QWEN.md file.").
    GlobalSlot(HostType.QWEN, ".qwen/QWEN.md", "markdown"),
    # pi coding agent — added 2026-04-16 as the 9th host. pi.dev
    # loads ~/.pi/AGENTS.md or ~/.pi/CLAUDE.md as global context.
    GlobalSlot(HostType.PI, ".pi/AGENTS.md", "markdown"),
]


# Auto-memory shared location — every CLI's memory/ path symlinks here.
# v5 (2026-04-14): canonical is $MAKAKOO_HOME/data/auto-memory. $HARVEY_HOME
# still resolves to the same directory via the ~/MAKAKOO symlink, so existing
# symlinks that embed "/HARVEY/" in their target keep working without change.
def _platform_home() -> Path:
    for env_var in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(env_var)
        if val:
            return Path(os.path.expanduser(val))
    return Path.home() / "MAKAKOO"


AUTO_MEMORY_CANONICAL = _platform_home() / "data" / "auto-memory"

# v5: both Claude Code project slugs are listed so whether Sebastian opens
# a session from ~/MAKAKOO or ~/MAKAKOO the same memory tree is available.
# The project slug is derived by Claude Code from the literal cwd, not the
# real path of the symlink.
AUTO_MEMORY_SYMLINKS: List[Path] = [
    Path.home() / ".claude" / "projects" / "-Users-sebastian-HARVEY" / "memory",
    Path.home() / ".claude" / "projects" / "-Users-sebastian-MAKAKOO" / "memory",
    Path.home() / ".gemini" / "memory",
    Path.home() / ".codex" / "memory",
    Path.home() / ".config" / "opencode" / "memory",
    Path.home() / ".vibe" / "memory",
    Path.home() / ".cursor" / "memory",
    # v7 (2026-04-14): Qwen Code joined the family. Qwen's native memory
    # lives in ~/.qwen/QWEN.md (single-file convention inherited from the
    # Gemini CLI fork — DEFAULT_CONTEXT_FILENAME = "QWEN.md"). This
    # `memory/` symlink is a Makakoo convention, NOT a Qwen-native feature,
    # matching the pattern we use for every other host: it points at the
    # shared auto-memory tree so Harvey's cross-session lessons are
    # visible to Qwen the same way they are to the other 6 CLIs.
    Path.home() / ".qwen" / "memory",
    # v9 (2026-04-16): pi coding agent (pi.dev) joins as the 9th host.
    Path.home() / ".pi" / "memory",
]


# v8 (2026-04-14): dynamic host registry.
#
# Hosts added via `harvey onboard <name>` land in
# $MAKAKOO_HOME/config/cli_registry.json with their slot path, memory
# symlink path, display name, and backend metadata. At module import
# time we merge those into the iteration lists so subsequent
# `harvey infect --global` / `harvey status` runs treat them as first-
# class alongside the built-in 7 hosts.
#
# The registry is read-mostly. `harvey onboard` is the only writer.

_REGISTRY_REL_PATH = "config/cli_registry.json"


def _registry_path() -> Path:
    """Absolute path to the dynamic-host registry file."""
    return _platform_home() / _REGISTRY_REL_PATH


def load_cli_registry() -> Dict[str, Any]:
    """Return the parsed registry dict, or an empty stub if no file / corrupt.

    Never raises — onboarding is best-effort and the built-in SLOTS list
    must keep working even if the registry file is garbage.
    """
    path = _registry_path()
    if not path.is_file():
        return {"version": 1, "hosts": []}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return {"version": 1, "hosts": []}
        if "hosts" not in data or not isinstance(data["hosts"], list):
            data["hosts"] = []
        return data
    except (OSError, json.JSONDecodeError):
        return {"version": 1, "hosts": []}


def save_cli_registry(data: Dict[str, Any]) -> None:
    """Atomically write the registry. Only `harvey onboard` calls this."""
    path = _registry_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def _dynamic_slots() -> List[GlobalSlot]:
    """Translate the registry file into runtime GlobalSlot entries.

    Each registry entry becomes one DYNAMIC-typed GlobalSlot whose
    `display_name` carries the real CLI name. The rel_path is built
    from `config_dir` + `context_file` so future edits to the registry
    are reflected without touching this module.
    """
    data = load_cli_registry()
    out: List[GlobalSlot] = []
    for entry in data.get("hosts", []):
        if not isinstance(entry, dict):
            continue
        config_dir = entry.get("config_dir")
        context_file = entry.get("context_file")
        name = entry.get("name")
        if not (config_dir and context_file and name):
            continue
        rel_path = "{}/{}".format(config_dir.strip("/"), context_file)
        out.append(
            GlobalSlot(
                host=HostType.DYNAMIC,
                rel_path=rel_path,
                format="markdown",
                display_name=str(name),
            )
        )
    return out


def _dynamic_memory_symlinks() -> List[Path]:
    """Translate the registry into runtime memory-symlink paths."""
    data = load_cli_registry()
    out: List[Path] = []
    for entry in data.get("hosts", []):
        if not isinstance(entry, dict):
            continue
        sym = entry.get("memory_symlink")
        if not sym:
            continue
        out.append(Path.home() / str(sym).lstrip("/"))
    return out


def _all_slots() -> List[GlobalSlot]:
    """Built-in SLOTS plus any dynamically-registered hosts, deduped by rel_path."""
    seen = set()
    result: List[GlobalSlot] = []
    for slot in SLOTS:
        if slot.rel_path in seen:
            continue
        seen.add(slot.rel_path)
        result.append(slot)
    for slot in _dynamic_slots():
        if slot.rel_path in seen:
            continue
        seen.add(slot.rel_path)
        result.append(slot)
    return result


def _all_memory_symlinks() -> List[Path]:
    """Built-in AUTO_MEMORY_SYMLINKS plus any dynamically-registered symlinks."""
    seen = set()
    result: List[Path] = []
    for p in AUTO_MEMORY_SYMLINKS:
        if p in seen:
            continue
        seen.add(p)
        result.append(p)
    for p in _dynamic_memory_symlinks():
        if p in seen:
            continue
        seen.add(p)
        result.append(p)
    return result


# v4: shared skill tree — dispatcher canonical + filesystem links for 3 CLIs.
#
# Intentionally excluded from SKILLS_SYMLINKS:
#   ~/.claude/skills/   — gstack owns it (132 auto-managed symlinks + real
#                         package dir). Replacing it would corrupt gstack.
#                         Claude uses `harvey skill run` for unified access.
#   ~/.codex/skills/    — get-shit-done manages this with per-CLI tailored
#                         SKILL.md content. Different from cursor by design.
#   ~/.cursor/skills/   — same (gsd-managed with cursor-specific variants).
#
# Every CLI, including the excluded three, still gets unified skill access
# via the `harvey skill run` dispatcher that resolves from SKILLS_CANONICAL.
SKILLS_CANONICAL = _platform_home() / "skills-shared"

SKILLS_SYMLINKS: List[Path] = [
    Path.home() / ".gemini" / "skills",
    Path.home() / ".config" / "opencode" / "skills",
    Path.home() / ".vibe" / "skills",
]


@dataclass
class SkillVerifyReport:
    """v4 skill symlink verification report."""
    canonical_path: Path
    canonical_exists: bool
    canonical_skill_count: int
    symlinks: List["SymlinkStatus"]

    def is_clean(self) -> bool:
        return (
            self.canonical_exists
            and self.canonical_skill_count > 0
            and all(s.ok for s in self.symlinks)
        )


# ─── Block parsing helpers ─────────────────────────────────────


def _find_markdown_block(text: str) -> Optional[Tuple[int, int, str]]:
    """Return (start, end, version) of an existing block, or None."""
    m = _BLOCK_RE.search(text)
    if not m:
        return None
    block_text = m.group(0)
    vm = re.search(r"START\s+v(\S+?)\s*-->", block_text)
    version = vm.group(1) if vm else "?"
    return (m.start(), m.end(), version)


def _strip_markdown_block(text: str) -> str:
    """Remove the fenced block entirely, collapsing surrounding blank lines."""
    stripped = _BLOCK_RE.sub("\n", text)
    # Normalize excessive trailing whitespace
    return stripped.rstrip() + "\n" if stripped.strip() else ""


# ─── Per-host post-install hooks ───────────────────────────────
#
# Some CLIs need extra setup beyond writing the bootstrap block:
# shell aliases, skill loading, config tweaks. These hooks run
# after the block is written by install_one().

_PI_ALIAS_MARKER = "# Makakoo OS — Pi CLI infection"


def _pi_alias_line(path: Path) -> str:
    """Build the full pi alias with Makakoo bootstrap + switchai provider + skills."""
    return (
        f'alias pi="pi --append-system-prompt {path} '
        f'--skill ~/.lope/commands/opencode '
        f'--provider switchai '
        f'--model minimax:MiniMax-M2.7"'
    )


def _pi_post_install(slot: GlobalSlot, path: Path) -> None:
    """Pi-specific setup: ~/.zshrc alias + memory symlink.

    The alias injects --append-system-prompt, --skill, --provider,
    --model, and --api-key so Pi auto-loads Makakoo bootstrap, lope
    commands, and switchAILocal (MiniMax) on every launch — even after
    npm install -g updates that wipe settings.json.
    """
    home = Path.home()

    # 1. Memory symlink — canonical auto-memory
    mem_link = home / ".pi" / "memory"
    canon = AUTO_MEMORY_CANONICAL
    if not mem_link.exists():
        try:
            mem_link.symlink_to(canon)
        except OSError:
            pass  # parent dir may not exist yet; sync_memory_symlinks will retry
    elif mem_link.is_symlink() and mem_link.resolve() != canon.resolve():
        mem_link.unlink()
        mem_link.symlink_to(canon)

    # 2. Shell alias in ~/.zshrc — idempotent, survives npm reinstalls
    alias_line = _pi_alias_line(path)
    zshrc = home / ".zshrc"
    if zshrc.exists():
        existing = zshrc.read_text(encoding="utf-8")
        if _PI_ALIAS_MARKER not in existing:
            block = (
                f"\n{_PI_ALIAS_MARKER}\n"
                f'export AIL_API_KEY="sk-test-123"\n'
                f"{alias_line}\n"
            )
            with open(zshrc, "a", encoding="utf-8") as f:
                f.write(block)
        else:
            new_content = re.sub(r'alias pi=".*"', alias_line, existing)
            # Ensure AIL_API_KEY export exists (may be missing from older infections)
            if "AIL_API_KEY" not in existing:
                new_content = new_content.replace(
                    _PI_ALIAS_MARKER,
                    f'{_PI_ALIAS_MARKER}\nexport AIL_API_KEY="sk-test-123"',
                )
            if new_content != existing:
                zshrc.write_text(new_content, encoding="utf-8")


# Map of host types to their post-install hook (if any).
_POST_INSTALL_HOOKS: Dict[HostType, callable] = {
    HostType.PI: _pi_post_install,
}


def _upsert_markdown_block(text: str, new_block: str) -> Tuple[str, SlotStatus, str]:
    """Insert or replace the fenced block. Returns (new_text, status, prior_version)."""
    found = _find_markdown_block(text)
    if found is None:
        # Append to end with a blank-line separator.
        if text and not text.endswith("\n"):
            text = text + "\n"
        if text and not text.endswith("\n\n"):
            text = text + "\n"
        return (text + new_block, SlotStatus.INSTALLED, "")
    start, end, prior_version = found
    if prior_version == BLOCK_VERSION:
        return (text, SlotStatus.UNCHANGED, prior_version)
    new_text = text[:start] + new_block + text[end:]
    return (new_text, SlotStatus.UPDATED, prior_version)


def _find_opencode_entry(instructions: list) -> Optional[int]:
    """Return the index of a tagged harvey bootstrap entry in an instructions list, or None."""
    for i, entry in enumerate(instructions):
        if isinstance(entry, str) and _JSON_TAG_FINGERPRINT in entry[:40]:
            return i
    return None


def _opencode_entry_version(entry: str) -> str:
    """Pull the version token out of a tagged instruction string, or '?'."""
    m = re.match(r"\[harvey:infect-global v(\S+?)\]", entry)
    return m.group(1) if m else "?"


# ─── Atomic write ──────────────────────────────────────────────


def _atomic_write(path: Path, content: str) -> None:
    """Write content to path atomically via temp file + os.replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".infect_global.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(content)
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


# ─── Public API ────────────────────────────────────────────────


class GlobalInfector:
    """Installs / revokes / inspects the Harvey Bootstrap Block across all
    global CLI slots. `home` is injectable for testing."""

    def __init__(self, home: Optional[Path] = None):
        self.home = Path(home or os.path.expanduser("~"))

    # -- public operations --

    def install_all(self, *, dry_run: bool = False, skip_missing: bool = True) -> List[GlobalInfectResult]:
        results = []
        for slot in _all_slots():
            r = self.install_one(slot, dry_run=dry_run, skip_missing=skip_missing)
            # Stamp the slot's display_name onto the result so CLI output
            # uses the real host name for dynamically-registered CLIs
            # instead of the generic "dynamic" enum value.
            r.display_name = slot.display_name
            results.append(r)
        return results

    def revoke_all(self, *, dry_run: bool = False) -> List[GlobalInfectResult]:
        results = []
        for slot in _all_slots():
            r = self.revoke_one(slot, dry_run=dry_run)
            r.display_name = slot.display_name
            results.append(r)
        return results

    def status_all(self) -> List[GlobalInfectResult]:
        results = []
        for slot in _all_slots():
            r = self.status_one(slot)
            r.display_name = slot.display_name
            results.append(r)
        return results

    def audit_all(self) -> "AuditReport":
        """Diff pre-infect content across all slots. v3 feature.

        Returns an AuditReport listing each slot's non-block content +
        byte count, and flags any slot whose pre-infect content contains
        known-stale fact patterns (year-stale CV path, old career-manager
        location, legacy Brain path, etc).
        """
        drifts: List[DriftFinding] = []
        snapshots: List[SlotSnapshot] = []
        for slot in _all_slots():
            path = self.home / slot.rel_path
            if not path.exists():
                snapshots.append(SlotSnapshot(slot.host, path, pre_block="", block_version="", pre_block_bytes=0))
                continue
            try:
                if slot.format == "markdown":
                    text = path.read_text(encoding="utf-8")
                    pre, version = _split_markdown_pre_block(text)
                elif slot.format == "opencode_json":
                    # OpenCode JSON: "pre-block" is the entire opencode.json
                    # content minus the harvey bootstrap instructions entry.
                    data = json.loads(path.read_text(encoding="utf-8"))
                    pre, version = _describe_opencode_non_harvey(data)
                else:
                    pre, version = "", ""
            except (OSError, json.JSONDecodeError) as e:
                snapshots.append(SlotSnapshot(slot.host, path, pre_block=f"<error: {e}>",
                                              block_version="", pre_block_bytes=0))
                continue

            snapshots.append(SlotSnapshot(
                host=slot.host,
                path=path,
                pre_block=pre,
                block_version=version,
                pre_block_bytes=len(pre.encode("utf-8")),
            ))
            for pattern, label in _STALE_PATTERNS:
                if pattern.search(pre):
                    drifts.append(DriftFinding(
                        host=slot.host,
                        path=path,
                        pattern_label=label,
                        excerpt=_excerpt_for(pre, pattern, maxlen=200),
                    ))
        return AuditReport(snapshots=snapshots, drifts=drifts)

    def verify_all(self) -> "VerifyReport":
        """Assert every slot has current v3 block and every memory symlink
        resolves to the canonical auto-memory directory. v3 feature.

        Returns a VerifyReport with per-slot status + per-symlink status.
        A fully-clean machine has all slots in UNCHANGED (already v3) and
        all symlinks resolving to AUTO_MEMORY_CANONICAL.
        """
        slot_results = self.status_all()

        symlink_results: List[SymlinkStatus] = []
        for link_path in _all_memory_symlinks():
            if not link_path.exists() and not link_path.is_symlink():
                symlink_results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                      reason="missing"))
                continue
            if not link_path.is_symlink():
                symlink_results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                      reason="not a symlink (directory or file)"))
                continue
            try:
                target = link_path.resolve()
            except OSError as e:
                symlink_results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                      reason=f"resolve error: {e}"))
                continue
            canonical = AUTO_MEMORY_CANONICAL.resolve() if AUTO_MEMORY_CANONICAL.exists() else AUTO_MEMORY_CANONICAL
            ok = target == canonical
            symlink_results.append(SymlinkStatus(
                link_path, target=target, ok=ok,
                reason="" if ok else f"target is {target}, expected {canonical}",
            ))

        return VerifyReport(
            slots=slot_results,
            symlinks=symlink_results,
            canonical_memory_exists=AUTO_MEMORY_CANONICAL.exists(),
        )

    def sync_memory_symlinks(self, *, dry_run: bool = False) -> List["SymlinkStatus"]:
        """Create or repair the per-CLI memory symlinks so each CLI
        memory path points at AUTO_MEMORY_CANONICAL. v3 feature.

        Idempotent: if the symlink already points at the canonical dir,
        nothing changes. If a regular directory is at the path, this
        refuses to touch it and reports the conflict — the user must
        manually move the content into the canonical location first.
        """
        # Ensure the canonical directory exists before creating links
        AUTO_MEMORY_CANONICAL.mkdir(parents=True, exist_ok=True)

        results: List[SymlinkStatus] = []
        canonical = AUTO_MEMORY_CANONICAL.resolve()

        for link_path in _all_memory_symlinks():
            if link_path.is_symlink():
                try:
                    current_target = link_path.resolve()
                except OSError as e:
                    results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                  reason=f"resolve error: {e}"))
                    continue
                if current_target == canonical:
                    results.append(SymlinkStatus(link_path, target=current_target, ok=True))
                    continue
                # Wrong target — repoint
                if not dry_run:
                    link_path.unlink()
                    link_path.parent.mkdir(parents=True, exist_ok=True)
                    link_path.symlink_to(AUTO_MEMORY_CANONICAL)
                results.append(SymlinkStatus(link_path, target=canonical, ok=True,
                                              reason="repointed (was elsewhere)"))
                continue
            if link_path.exists():
                # Regular directory or file — do NOT destroy
                results.append(SymlinkStatus(link_path, target=None, ok=False,
                                              reason="exists as directory/file, not symlink (refused to touch)"))
                continue
            # Path does not exist — create fresh symlink
            if not dry_run:
                link_path.parent.mkdir(parents=True, exist_ok=True)
                link_path.symlink_to(AUTO_MEMORY_CANONICAL)
            results.append(SymlinkStatus(link_path, target=canonical, ok=True,
                                          reason="created"))
        return results

    # -- v4: skill symlinks --

    def _skill_dir_is_backup_safe(self, dir_path: Path) -> Tuple[bool, str]:
        """Return (safe_to_backup, reason).

        A directory is 'backup-safe' when all its immediate children are
        representable inside the canonical skills tree — either the same
        name exists under canonical (any subtree) OR the child is empty.
        If not safe, the sync refuses to touch the dir.

        Near-empty CLI skill dirs (gemini, opencode, vibe, etc.) pass
        trivially. A dir populated with unknown skills refuses.
        """
        try:
            children = list(dir_path.iterdir())
        except OSError as e:
            return False, f"cannot enumerate: {e}"
        if not children:
            return True, "empty"
        canonical_names: set[str] = set()
        for sub in ("gstack", "harvey", "user"):
            sub_root = SKILLS_CANONICAL / sub
            if not sub_root.exists():
                continue
            if sub == "harvey":
                for category in sub_root.iterdir():
                    if category.is_dir():
                        canonical_names.update(c.name for c in category.iterdir() if c.is_dir())
            else:
                canonical_names.update(c.name for c in sub_root.iterdir() if c.is_dir())
        unknown = [c.name for c in children if c.is_dir() and c.name not in canonical_names]
        if unknown:
            return False, f"unknown skills present: {', '.join(unknown[:5])}"
        return True, f"all {len(children)} children present in canonical"

    def audit_skills(self) -> "SkillVerifyReport":
        """Snapshot the current state of every skill symlink slot.

        Reuses SkillVerifyReport — audit = snapshot, verify = assertion.
        """
        return self.verify_skills()

    def verify_skills(self) -> "SkillVerifyReport":
        """Assert every skill symlink resolves to canonical + canonical is healthy."""
        canonical_exists = SKILLS_CANONICAL.is_dir()
        skill_count = 0
        if canonical_exists:
            for sub in ("gstack", "harvey", "user"):
                p = SKILLS_CANONICAL / sub
                if p.exists():
                    skill_count += sum(1 for _ in p.rglob("SKILL.md"))
        canonical_real = SKILLS_CANONICAL.resolve() if canonical_exists else None

        symlinks: List[SymlinkStatus] = []
        for link_path in SKILLS_SYMLINKS:
            if not link_path.exists() and not link_path.is_symlink():
                symlinks.append(SymlinkStatus(link_path, target=None, ok=False,
                                              reason="missing"))
                continue
            if not link_path.is_symlink():
                symlinks.append(SymlinkStatus(link_path, target=None, ok=False,
                                              reason="exists as real dir (not linked)"))
                continue
            try:
                current = link_path.resolve()
            except OSError as e:
                symlinks.append(SymlinkStatus(link_path, target=None, ok=False,
                                              reason=f"resolve error: {e}"))
                continue
            if current == canonical_real:
                symlinks.append(SymlinkStatus(link_path, target=current, ok=True))
            else:
                symlinks.append(SymlinkStatus(link_path, target=current, ok=False,
                                              reason=f"wrong target: {current}"))

        return SkillVerifyReport(
            canonical_path=SKILLS_CANONICAL,
            canonical_exists=canonical_exists,
            canonical_skill_count=skill_count,
            symlinks=symlinks,
        )

    def sync_skill_symlinks(self, *, dry_run: bool = False) -> List["SymlinkStatus"]:
        """Create or repair the per-CLI skill symlinks so each CLI
        skills/ path points at SKILLS_CANONICAL. v4 feature.

        Data-safe: refuses to destroy real directories containing
        content not in canonical. If a real dir passes the safety check,
        it is RENAMED to `<path>.v4-backup-<ts>` (not deleted), and a
        symlink created in its place. Nothing is ever removed.
        """
        SKILLS_CANONICAL.mkdir(parents=True, exist_ok=True)

        results: List[SymlinkStatus] = []
        canonical = SKILLS_CANONICAL.resolve()
        import time  # local import: only needed for backup suffix

        for link_path in SKILLS_SYMLINKS:
            if link_path.is_symlink():
                try:
                    current_target = link_path.resolve()
                except OSError as e:
                    results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                  reason=f"resolve error: {e}"))
                    continue
                if current_target == canonical:
                    results.append(SymlinkStatus(link_path, target=current_target, ok=True))
                    continue
                if not dry_run:
                    link_path.unlink()
                    link_path.parent.mkdir(parents=True, exist_ok=True)
                    link_path.symlink_to(SKILLS_CANONICAL)
                results.append(SymlinkStatus(link_path, target=canonical, ok=True,
                                              reason="repointed (was elsewhere)"))
                continue
            if link_path.exists():
                safe, reason = self._skill_dir_is_backup_safe(link_path)
                if not safe:
                    results.append(SymlinkStatus(link_path, target=None, ok=False,
                                                  reason=f"refused: {reason}"))
                    continue
                if not dry_run:
                    ts = time.strftime("%Y%m%d-%H%M%S")
                    backup_path = link_path.with_name(f"{link_path.name}.v4-backup-{ts}")
                    link_path.rename(backup_path)
                    link_path.parent.mkdir(parents=True, exist_ok=True)
                    link_path.symlink_to(SKILLS_CANONICAL)
                results.append(SymlinkStatus(link_path, target=canonical, ok=True,
                                              reason=f"backed up + linked ({reason})"))
                continue
            if not dry_run:
                link_path.parent.mkdir(parents=True, exist_ok=True)
                link_path.symlink_to(SKILLS_CANONICAL)
            results.append(SymlinkStatus(link_path, target=canonical, ok=True,
                                          reason="created"))
        return results

    # -- per-slot operations --

    def install_one(
        self,
        slot: GlobalSlot,
        *,
        dry_run: bool = False,
        skip_missing: bool = True,
    ) -> GlobalInfectResult:
        path = self.home / slot.rel_path
        try:
            if slot.format == "markdown":
                result = self._install_markdown(slot, path, dry_run=dry_run, skip_missing=skip_missing)
            elif slot.format == "opencode_json":
                result = self._install_opencode(slot, path, dry_run=dry_run, skip_missing=skip_missing)
            else:
                result = GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=f"unknown format {slot.format}")
        except OSError as e:
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=str(e))

        # Post-install hook: per-host extra setup (aliases, skills, etc.)
        # Runs even on UNCHANGED — the alias/symlink may be missing even if
        # the block is already current. Hook is idempotent.
        hook = _POST_INSTALL_HOOKS.get(slot.host)
        if hook and not dry_run and result.status not in (SlotStatus.ERROR, SlotStatus.SKIPPED):
            try:
                hook(slot, path)
            except Exception:
                pass  # best-effort; block install already succeeded

        return result

    def revoke_one(self, slot: GlobalSlot, *, dry_run: bool = False) -> GlobalInfectResult:
        path = self.home / slot.rel_path
        try:
            if not path.exists():
                return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)
            if slot.format == "markdown":
                return self._revoke_markdown(slot, path, dry_run=dry_run)
            if slot.format == "opencode_json":
                return self._revoke_opencode(slot, path, dry_run=dry_run)
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=f"unknown format {slot.format}")
        except OSError as e:
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=str(e))

    def status_one(self, slot: GlobalSlot) -> GlobalInfectResult:
        path = self.home / slot.rel_path
        if not path.exists():
            return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)
        try:
            if slot.format == "markdown":
                text = path.read_text(encoding="utf-8")
                found = _find_markdown_block(text)
                if found is None:
                    return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)
                _, _, version = found
                status = SlotStatus.UNCHANGED if version == BLOCK_VERSION else SlotStatus.UPDATED
                return GlobalInfectResult(slot.host, path, status, version=version)
            if slot.format == "opencode_json":
                data = json.loads(path.read_text(encoding="utf-8"))
                instructions = data.get("instructions") or []
                idx = _find_opencode_entry(instructions)
                if idx is None:
                    return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)
                version = _opencode_entry_version(instructions[idx])
                status = SlotStatus.UNCHANGED if version == BLOCK_VERSION else SlotStatus.UPDATED
                return GlobalInfectResult(slot.host, path, status, version=version)
        except (OSError, json.JSONDecodeError) as e:
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=str(e))
        return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error="unreachable")

    # -- markdown internals --

    def _install_markdown(
        self,
        slot: GlobalSlot,
        path: Path,
        *,
        dry_run: bool,
        skip_missing: bool,
    ) -> GlobalInfectResult:
        if not path.parent.exists() and skip_missing and not path.exists():
            # Parent dir missing → the CLI is probably not installed at all.
            return GlobalInfectResult(slot.host, path, SlotStatus.SKIPPED,
                                      error=f"{path.parent} does not exist")

        text = path.read_text(encoding="utf-8") if path.exists() else ""
        new_block = _render_markdown_block()
        new_text, status, prior_version = _upsert_markdown_block(text, new_block)

        if status == SlotStatus.UNCHANGED or dry_run:
            if dry_run and status != SlotStatus.UNCHANGED:
                return GlobalInfectResult(slot.host, path, SlotStatus.DRY_RUN,
                                          version=BLOCK_VERSION, previous_version=prior_version)
            return GlobalInfectResult(slot.host, path, status,
                                      version=BLOCK_VERSION, previous_version=prior_version)

        _atomic_write(path, new_text)
        return GlobalInfectResult(slot.host, path, status,
                                  version=BLOCK_VERSION, previous_version=prior_version)

    def _revoke_markdown(self, slot: GlobalSlot, path: Path, *, dry_run: bool) -> GlobalInfectResult:
        text = path.read_text(encoding="utf-8")
        found = _find_markdown_block(text)
        if found is None:
            return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)
        _, _, prior_version = found
        new_text = _strip_markdown_block(text)
        if dry_run:
            return GlobalInfectResult(slot.host, path, SlotStatus.DRY_RUN, previous_version=prior_version)
        _atomic_write(path, new_text)
        return GlobalInfectResult(slot.host, path, SlotStatus.REVOKED, previous_version=prior_version)

    # -- opencode internals --

    def _install_opencode(
        self,
        slot: GlobalSlot,
        path: Path,
        *,
        dry_run: bool,
        skip_missing: bool,
    ) -> GlobalInfectResult:
        if not path.parent.exists() and skip_missing and not path.exists():
            return GlobalInfectResult(slot.host, path, SlotStatus.SKIPPED,
                                      error=f"{path.parent} does not exist")

        data: dict = {}
        if path.exists():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
            except json.JSONDecodeError as e:
                return GlobalInfectResult(slot.host, path, SlotStatus.ERROR,
                                          error=f"existing config is invalid JSON: {e}")
            if not isinstance(data, dict):
                return GlobalInfectResult(slot.host, path, SlotStatus.ERROR,
                                          error="opencode config root is not an object")

        instructions = data.get("instructions")
        if instructions is None:
            instructions = []
            data["instructions"] = instructions
        if not isinstance(instructions, list):
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR,
                                      error="opencode `instructions` is not an array")

        new_entry = _render_opencode_instruction()
        idx = _find_opencode_entry(instructions)

        if idx is None:
            instructions.append(new_entry)
            status = SlotStatus.INSTALLED
            prior_version = ""
        else:
            prior_version = _opencode_entry_version(instructions[idx])
            if prior_version == BLOCK_VERSION and instructions[idx] == new_entry:
                return GlobalInfectResult(slot.host, path, SlotStatus.UNCHANGED,
                                          version=BLOCK_VERSION, previous_version=prior_version)
            instructions[idx] = new_entry
            status = SlotStatus.UPDATED

        if dry_run:
            return GlobalInfectResult(slot.host, path, SlotStatus.DRY_RUN,
                                      version=BLOCK_VERSION, previous_version=prior_version)

        _atomic_write(path, json.dumps(data, indent=2) + "\n")
        return GlobalInfectResult(slot.host, path, status,
                                  version=BLOCK_VERSION, previous_version=prior_version)

    def _revoke_opencode(self, slot: GlobalSlot, path: Path, *, dry_run: bool) -> GlobalInfectResult:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR, error=str(e))
        if not isinstance(data, dict):
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR,
                                      error="opencode config root is not an object")
        instructions = data.get("instructions") or []
        if not isinstance(instructions, list):
            return GlobalInfectResult(slot.host, path, SlotStatus.ERROR,
                                      error="opencode `instructions` is not an array")

        idx = _find_opencode_entry(instructions)
        if idx is None:
            return GlobalInfectResult(slot.host, path, SlotStatus.ABSENT)

        prior_version = _opencode_entry_version(instructions[idx])
        instructions.pop(idx)
        # If the array is now empty AND we originally created it, leave it
        # as [] rather than deleting — lets future reinstalls stay simple.

        if dry_run:
            return GlobalInfectResult(slot.host, path, SlotStatus.DRY_RUN, previous_version=prior_version)

        _atomic_write(path, json.dumps(data, indent=2) + "\n")
        return GlobalInfectResult(slot.host, path, SlotStatus.REVOKED, previous_version=prior_version)
