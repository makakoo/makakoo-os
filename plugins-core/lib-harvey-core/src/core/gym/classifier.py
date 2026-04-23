"""
Layer 2 of Harvey's Mascot GYM — error classifier + clustering.

Turns the raw jsonl output of Layer 1 into labeled, deduplicated clusters.
Rules-based by design: this runs hourly, must be deterministic, and must
never call an LLM (hot path + hermetic reproducibility required for the
morning report).

Classification labels:
    ENVIRONMENTAL — network / auth / infra issue, not Harvey's fault
    USER          — user typo / wrong flag / bad path / permission denied
    SKILL         — something Harvey could actually learn from (doc/skill-level)
    CODE          — code-level bug: ImportError, AttributeError, KeyError, TypeError,
                     traceback with a fixable source location in a Python file under Harvey's control
    UNKNOWN       — none of the above (conservative default)

Dedup strategy:
    hash = sha256(source + normalized_cmd + normalized_stderr_first_line)
    Normalizer strips absolute paths, digit runs, and hex runs so that
    "rm /tmp/x1" and "rm /tmp/x9999" collapse to the same cluster.

Cluster window:
    All entries sharing a hash within the same UTC day form one cluster.
    Cross-day clustering is out of scope — Layer 3 makes decisions per-day.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
ERRORS_DIR = Path(HARVEY_HOME) / "data" / "errors"
IMPROVEMENTS_DIR = Path(HARVEY_HOME) / "data" / "improvements"

CLUSTERS_FILENAME = "clustered.json"
DEFAULT_TOP_N = 30


def _load_blocklist() -> set:
    """Return cluster_ids suppressed by the weekly rollup. Empty on any failure."""
    path = IMPROVEMENTS_DIR / "blocklist.json"
    if not path.exists():
        return set()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return set(data.get("cluster_ids", []))
    except (OSError, json.JSONDecodeError):
        return set()


class ErrorClass(str, Enum):
    ENVIRONMENTAL = "environmental"
    SKILL = "skill"
    CODE = "code"
    USER = "user"
    UNKNOWN = "unknown"


# -----------------------------------------------------------------------------
# Classification rules — ordered by priority. First rule that matches wins.
# -----------------------------------------------------------------------------
#
# Keep the regex surface small: if a pattern is overly loose it poisons
# every day's output. When in doubt, make it stricter and let the fallback
# route it to UNKNOWN.

_ENVIRONMENTAL_PATTERNS = [
    re.compile(r"\bconnection\s+refused\b", re.I),
    re.compile(r"\bconnect(?:ion)?\s+timed?\s*out\b", re.I),
    re.compile(r"\bdns\s+resolution\b", re.I),
    re.compile(r"\bname\s+or\s+service\s+not\s+known\b", re.I),
    re.compile(r"\btemporary\s+failure\s+in\s+name\s+resolution\b", re.I),
    re.compile(r"\bno\s+route\s+to\s+host\b", re.I),
    re.compile(r"\bnetwork\s+is\s+unreachable\b", re.I),
    re.compile(r"\bread\s+timeout(?:ed)?\b", re.I),
    re.compile(r"\bhttp\s*5\d{2}\b"),
    re.compile(r"\bhttperror:\s*5\d{2}\b", re.I),
    re.compile(r"\bssl:\s*certificate\s+verify\s+failed\b", re.I),
    re.compile(r"\btls\s+handshake\s+failure\b", re.I),
    re.compile(r"\bcurl:\s*\(6\)", re.I),           # Couldn't resolve host
    re.compile(r"\bcurl:\s*\(7\)", re.I),           # Failed to connect
    re.compile(r"\bcurl:\s*\(28\)", re.I),          # Timeout
    re.compile(r"\bswitchailocal\b.*\b(?:refused|unreachable|down)\b", re.I),
    re.compile(r"\bpod\s+config\s+not\s+ready\b", re.I),
    re.compile(r"\btunnel\s+daemon\s+already\s+running\b", re.I),
    re.compile(r"\blaunchctl\s+load\s+failed\b", re.I),
    re.compile(r"\bread-only\s+file\s+system\b", re.I),
    re.compile(r"\bno\s+space\s+left\s+on\s+device\b", re.I),
    re.compile(r"\baddress\s+already\s+in\s+use\b", re.I),
    re.compile(r"\bbroken\s+pipe\b", re.I),
]

_CODE_PATTERNS = [
    # Python exception types — strong signal of a fixable code bug
    re.compile(r"\bImportError\b"),
    re.compile(r"\bModuleNotFoundError\b"),
    re.compile(r"\bAttributeError\b"),
    re.compile(r"\bKeyError\b"),
    re.compile(r"\bTypeError\b"),
    re.compile(r"\bValueError\b"),
    re.compile(r"\bIndexError\b"),
    re.compile(r"\bRuntimeError\b"),
    re.compile(r"\bTimeoutError\b"),
    re.compile(r"\bConnectionError\b"),
    re.compile(r"\bFileNotFoundError\b"),
    re.compile(r"\bPermissionError\b"),
    re.compile(r"\bJSONDecodeError\b"),
    re.compile(r"\bOperationalError\b"),
    # Traceback file locations pointing inside Harvey's codebase
    re.compile(r"File \"(.*[/\\](?:plugins-core|harvey-os|agents|makakoo-os)[^\"]*\.py)\""),
    re.compile(r"  File \"(.*[/\\]core[/\\][^\"]*\.py)\""),
    re.compile(r"    raise \w+Error"),
]

_USER_PATTERNS = [
    re.compile(r"\bunknown\s+(?:option|flag|argument|command)\b", re.I),
    re.compile(r"\bunrecognized\s+(?:option|argument)\b", re.I),
    re.compile(r"\binvalid\s+(?:option|argument|flag|choice)\b", re.I),
    re.compile(r"\bpermission\s+denied\b", re.I),
    re.compile(r"\busage:\s", re.I),
    re.compile(r"\bmissing\s+(?:required|positional)\s+argument\b", re.I),
    re.compile(r"\bno\s+such\s+file\s+or\s+directory\b", re.I),
    re.compile(r"\bfile\s+not\s+found\b", re.I),
    re.compile(r"\bnot\s+recognized\s+as\s+an?\s+internal\s+or\s+external\s+command\b", re.I),
    re.compile(r"\btry\s+['\"]?-?-?help['\"]?", re.I),
    re.compile(r"\bargparse\b.*\berror:\s", re.I),
    re.compile(r"\bcommand\s+not\s+found\b", re.I),
]

_CODE_FILE_RE = re.compile(r"File \"([^\"]*(?:plugins-core|harvey-os|agents|makakoo-os|core)[^\"]*\.py)\"")
_PATH_SEGMENT_RE = re.compile(r"(?:/[\w.\-@]+)+")
_DIGITS_RE = re.compile(r"\d+")
_HEX_RE = re.compile(r"\b[0-9a-f]{8,}\b", re.I)
_WHITESPACE_RE = re.compile(r"\s+")


@dataclass
class Cluster:
    """One deduplicated error signature with aggregated metadata."""
    cluster_id: str           # hash-derived id
    source: str
    count: int
    first_seen: str
    last_seen: str
    sample_cmd: str
    sample_stderr: str
    error_class: str
    skills_in_scope: List[str] = field(default_factory=list)
    error_ids: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


def classify_entry(entry: Dict[str, Any]) -> ErrorClass:
    """
    Apply the rule cascade. First match wins. Otherwise:
        - if skill_in_scope is set, return SKILL (actionable)
        - else return UNKNOWN

    Pre-labeled entries (error_class already set at write time, e.g. manual
    flags from `harvey flag`) bypass the cascade entirely — we trust the
    writer's intent over regex heuristics. This is load-bearing: without
    it, human-flagged feedback would get reclassified as UNKNOWN every time
    the classifier runs.
    """
    preset = entry.get("error_class")
    if preset:
        try:
            return ErrorClass(preset)
        except ValueError:
            pass  # unknown label — fall through to normal classification

    stderr = entry.get("stderr") or ""
    cmd = entry.get("cmd") or ""
    # Concatenate so rules can match either side
    text = f"{cmd}\n{stderr}"

    for pat in _ENVIRONMENTAL_PATTERNS:
        if pat.search(text):
            return ErrorClass.ENVIRONMENTAL

    for pat in _CODE_PATTERNS:
        if pat.search(text):
            return ErrorClass.CODE

    for pat in _USER_PATTERNS:
        if pat.search(text):
            return ErrorClass.USER

    if entry.get("skill_in_scope"):
        return ErrorClass.SKILL

    return ErrorClass.UNKNOWN


def _normalize(text: str) -> str:
    """Collapse volatile tokens so similar errors collide into one hash."""
    if not text:
        return ""
    text = _PATH_SEGMENT_RE.sub("<path>", text)
    text = _HEX_RE.sub("<hex>", text)
    text = _DIGITS_RE.sub("<n>", text)
    text = _WHITESPACE_RE.sub(" ", text).strip()
    return text


def cluster_hash(entry: Dict[str, Any]) -> str:
    """
    Deterministic signature for dedup.

    Only the first line of stderr participates, because multiline
    tracebacks vary on every run (line numbers, addresses, ids).
    """
    source = entry.get("source", "")
    cmd_norm = _normalize(entry.get("cmd", ""))
    stderr_first = (entry.get("stderr") or "").strip().split("\n", 1)[0]
    stderr_norm = _normalize(stderr_first)
    payload = f"{source}|{cmd_norm}|{stderr_norm}"
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16]


def _entry_ts(entry: Dict[str, Any]) -> str:
    return entry.get("ts", "")


def cluster_entries(entries: Iterable[Dict[str, Any]]) -> List[Cluster]:
    """
    Group entries by hash within the current batch. Returns a list
    sorted by count descending (ties broken by last_seen descending).
    """
    by_hash: Dict[str, Dict[str, Any]] = {}
    for entry in entries:
        entry_class = classify_entry(entry)
        h = cluster_hash(entry)
        slot = by_hash.setdefault(
            h,
            {
                "cluster_id": h,
                "source": entry.get("source", ""),
                "count": 0,
                "first_seen": _entry_ts(entry),
                "last_seen": _entry_ts(entry),
                "sample_cmd": entry.get("cmd", ""),
                "sample_stderr": (entry.get("stderr") or "").split("\n", 1)[0],
                "error_class": entry_class.value,
                "skills_in_scope": set(),
                "error_ids": [],
            },
        )
        slot["count"] += 1
        ts = _entry_ts(entry)
        if ts and ts < slot["first_seen"]:
            slot["first_seen"] = ts
        if ts and ts > slot["last_seen"]:
            slot["last_seen"] = ts
        skill = entry.get("skill_in_scope")
        if skill:
            slot["skills_in_scope"].add(skill)
        eid = entry.get("_id")
        if eid:
            slot["error_ids"].append(eid)

    clusters: List[Cluster] = []
    for slot in by_hash.values():
        clusters.append(
            Cluster(
                cluster_id=slot["cluster_id"],
                source=slot["source"],
                count=slot["count"],
                first_seen=slot["first_seen"],
                last_seen=slot["last_seen"],
                sample_cmd=slot["sample_cmd"],
                sample_stderr=slot["sample_stderr"],
                error_class=slot["error_class"],
                skills_in_scope=sorted(slot["skills_in_scope"]),
                error_ids=slot["error_ids"],
            )
        )
    clusters.sort(key=lambda c: (-c.count, c.last_seen), reverse=False)
    clusters.sort(key=lambda c: (-c.count, -ord(c.last_seen[0]) if c.last_seen else 0))
    # Stable sort by count desc, then last_seen desc (lexicographic ISO works fine)
    clusters.sort(key=lambda c: (-c.count, c.last_seen), reverse=False)
    return clusters


def _iter_day_entries(day_dir: Path) -> Iterable[Dict[str, Any]]:
    """Walk every *.jsonl under a day directory, yielding parsed entries."""
    if not day_dir.exists():
        return
    for jsonl in sorted(day_dir.glob("*.jsonl")):
        try:
            with open(jsonl, "r", encoding="utf-8") as f:
                for idx, line in enumerate(f):
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        entry = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    # synthesize a stable id so clusters can cite source lines
                    entry["_id"] = f"{jsonl.name}:{idx}"
                    yield entry
        except OSError:
            continue


_CODE_TARGET_RE = re.compile(
    r"File \"([^\"]*(?:plugins-core|harvey-os|agents|makakoo-os|core|skills)[^\"]*\.py)\""
)


def _extract_code_targets(entries: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """
    Extract fixable Python file locations from CODE-classified entries.

    Returns a list of targets, each with:
        file_path  — absolute path to the Python file with the bug
        line_no    — best-effort line number from traceback (or None)
        exception  — exception type (e.g. "ImportError", "KeyError")
        stderr     — full stderr for context
        count      — how many entries point to this same file
    """
    by_file: Dict[str, Dict[str, Any]] = {}

    for entry in entries:
        if classify_entry(entry) != ErrorClass.CODE:
            continue
        stderr = entry.get("stderr", "")

        # Extract file location from traceback
        path_match = _CODE_TARGET_RE.search(stderr)
        if not path_match:
            continue
        file_path = path_match.group(1)

        # Extract line number from traceback frame
        line_no = None
        frame_re = re.compile(r"\s+\d+\s+(.*\.py)", re.IGNORECASE)
        for m in frame_re.finditer(stderr):
            if m.group(1) == file_path:
                # Try to extract the number from "  File "..." line
                pass

        # Simple line number extraction: look for "line <N>" near the file mention
        line_re = re.compile(r"line (\d+)")
        line_match = None
        for m in line_re.finditer(stderr):
            line_match = int(m.group(1))
            break  # take first

        # Extract exception type
        exc_re = re.compile(r"(\w+Error|\w+Exception)\b")
        exc_match = exc_re.search(stderr)
        exc_type = exc_match.group(1) if exc_match else "Error"

        slot = by_file.setdefault(file_path, {
            "file_path": file_path,
            "exception": exc_type,
            "count": 0,
            "line_no": line_no,
            "stderr_samples": [],
            "error_ids": [],
        })
        slot["count"] += 1
        if len(slot["stderr_samples"]) < 3:
            slot["stderr_samples"].append(stderr[:500])
        eid = entry.get("_id")
        if eid:
            slot["error_ids"].append(eid)

    # Sort by count desc
    targets = sorted(by_file.values(), key=lambda t: -t["count"])
    return targets


def scan_day(date: Optional[str] = None, top_n: int = DEFAULT_TOP_N) -> Dict[str, Any]:
    """
    Read every jsonl under data/errors/<date>/, classify + cluster, write
    clustered.json in that same directory, and return a summary dict
    suitable for SANCHO result reporting.
    """
    date = date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    day_dir = ERRORS_DIR / date
    entries = list(_iter_day_entries(day_dir))

    summary = {
        "date": date,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "total_entries": len(entries),
        "by_class": {c.value: 0 for c in ErrorClass},
        "by_source": {},
        "clusters": [],
        "code_targets": [],  # populated after clustering: file paths with fixable errors
    }

    if not entries:
        _write_clusters(day_dir, summary)
        return summary

    for entry in entries:
        label = classify_entry(entry)
        summary["by_class"][label.value] += 1
        src = entry.get("source", "unknown")
        summary["by_source"][src] = summary["by_source"].get(src, 0) + 1

    clusters = cluster_entries(entries)
    blocklist = _load_blocklist()
    if blocklist:
        before = len(clusters)
        clusters = [c for c in clusters if c.cluster_id not in blocklist]
        summary["blocklist_suppressed"] = before - len(clusters)
    summary["cluster_count"] = len(clusters)
    summary["clusters"] = [c.to_dict() for c in clusters[:top_n]]

    # Extract code-level fixable targets for Layer 3's code patch generator
    code_targets = _extract_code_targets(entries)
    summary["code_targets"] = code_targets[:10]  # top 10 files

    _write_clusters(day_dir, summary)
    return summary


def _write_clusters(day_dir: Path, summary: Dict[str, Any]) -> None:
    day_dir.mkdir(parents=True, exist_ok=True)
    path = day_dir / CLUSTERS_FILENAME
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8")
    os.replace(tmp, path)  # atomic on POSIX
