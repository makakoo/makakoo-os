"""`harvey onboard <name>` — one-command CLI onboarding.

Replaces the 10-step manual dance Sebastian and Harvey went through to
add Qwen Code to Makakoo OS. Any future "we have a new teammate" moment
becomes a single line you can paste into any shell:

    harvey onboard <name>
    harvey onboard <name> --display-name "..." --config-dir .foo --context-file FOO.md
    harvey onboard <name> --dry-run

What it does, in order:

    1. Upsert the CLI into the dynamic registry at
       $MAKAKOO_HOME/config/cli_registry.json. `infect_global` reads
       this file on import and merges the entries into its SLOTS +
       AUTO_MEMORY_SYMLINKS iteration lists, so subsequent
       `harvey infect --global` runs include the new host automatically.

    2. Create the CLI's config directory if missing (`~/.<config-dir>`).

    3. Create the auto-memory symlink
       `~/.<config-dir>/memory → $MAKAKOO_HOME/data/auto-memory`
       so the CLI inherits Harvey's cross-session memory.

    4. Write / update the bootstrap block in the CLI's context file
       (`~/.<config-dir>/<CONTEXT_FILE>.md`) by running the in-process
       GlobalInfector. This is how the new CLI learns "you are Harvey,
       platform is Makakoo, use the omni tools for media, etc."

    5. Update the lope validator roster in `~/.lope/config.json`
       (unless `--no-lope`).

    6. Write a `project_<name>_onboarding.md` auto-memory file with
       the full wiring record.

    7. Append a one-line entry to the auto-memory `MEMORY.md` index.

    8. Append a welcome entry to today's Brain journal with a short
       note (full mascot round is left to manual journaling — this
       keeps the command fast and idempotent).

    9. Create a Brain page at `data/Brain/pages/<Display Name>.md`
       if none exists.

   10. Print a compact summary to stdout listing what was done vs.
       skipped (idempotent — re-running reports "already done" for
       each step that's already in place).

Every step is wrapped in a try/except so one failure never blocks the
rest of the pipeline. The final summary flags any step that errored so
the operator can re-run after fixing.

Python 3.9 compatible — no PEP 604 unions, no match statements.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

# Package-local imports — onboarding is a peer of infect_global
from core.orchestration import infect_global as ig


# ─── Data model ─────────────────────────────────────────────────


@dataclass
class HostSpec:
    """Full specification for a dynamically-onboarded CLI host."""

    name: str
    display_name: str
    config_dir: str            # e.g. ".qwen" (relative to $HOME, no leading slash)
    context_file: str          # e.g. "QWEN.md"
    memory_symlink: str        # e.g. ".qwen/memory"
    lope_validator: str = ""   # empty string = don't add to lope
    upstream_repo: str = ""
    backend_model: str = ""
    backend_url: str = ""
    onboarded_at: str = ""     # ISO timestamp, set at registry write

    def slot_rel_path(self) -> str:
        return "{}/{}".format(self.config_dir.strip("/"), self.context_file)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class StepResult:
    step: str
    status: str  # "ok", "unchanged", "skipped", "error"
    detail: str = ""


@dataclass
class OnboardResult:
    host: HostSpec
    dry_run: bool
    steps: List[StepResult] = field(default_factory=list)

    def ok(self) -> bool:
        return all(s.status != "error" for s in self.steps)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "host": self.host.to_dict(),
            "dry_run": self.dry_run,
            "steps": [asdict(s) for s in self.steps],
            "ok": self.ok(),
        }


# ─── Helpers ────────────────────────────────────────────────────


def _makakoo_home() -> Path:
    for env_var in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(env_var)
        if val:
            return Path(os.path.expanduser(val))
    return Path.home() / "MAKAKOO"


def _now_iso() -> str:
    return datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")


def _default_spec(
    name: str,
    *,
    display_name: Optional[str] = None,
    config_dir: Optional[str] = None,
    context_file: Optional[str] = None,
    lope_validator: Optional[str] = None,
    upstream_repo: str = "",
    backend_model: str = "",
    backend_url: str = "",
) -> HostSpec:
    """Build a HostSpec from a CLI name with sensible defaults.

    Defaults:
        display_name   = "<Name> Code" (title-cased)
        config_dir     = ".<name>"
        context_file   = "<NAME>.md"   (upper-case short name)
        memory_symlink = ".<name>/memory"
        lope_validator = "<name>"      (unless overridden or --no-lope)
    """
    slug = name.strip().lower()
    cfg_dir = (config_dir or ".{}".format(slug)).lstrip("/")
    ctx = context_file or "{}.md".format(slug.upper().replace("-", "_"))
    return HostSpec(
        name=slug,
        display_name=display_name or "{} Code".format(slug.title()),
        config_dir=cfg_dir,
        context_file=ctx,
        memory_symlink="{}/memory".format(cfg_dir),
        lope_validator=(slug if lope_validator is None else lope_validator),
        upstream_repo=upstream_repo,
        backend_model=backend_model,
        backend_url=backend_url,
    )


# ─── Individual steps ───────────────────────────────────────────


def _step_registry_upsert(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Add (or refresh) the host entry in cli_registry.json."""
    registry = ig.load_cli_registry()
    hosts = registry.get("hosts") or []
    existing_idx = None
    for i, entry in enumerate(hosts):
        if isinstance(entry, dict) and entry.get("name") == spec.name:
            existing_idx = i
            break

    new_entry = spec.to_dict()
    new_entry["onboarded_at"] = new_entry.get("onboarded_at") or _now_iso()

    if existing_idx is not None:
        # Preserve the original onboarded_at so we don't overwrite history
        prior = hosts[existing_idx]
        if isinstance(prior, dict) and prior.get("onboarded_at"):
            new_entry["onboarded_at"] = prior["onboarded_at"]
        if prior == new_entry:
            return StepResult("registry_upsert", "unchanged",
                              "host '{}' already in registry".format(spec.name))
        if dry_run:
            return StepResult("registry_upsert", "ok",
                              "would update existing entry '{}'".format(spec.name))
        hosts[existing_idx] = new_entry
    else:
        if dry_run:
            return StepResult("registry_upsert", "ok",
                              "would add new entry '{}'".format(spec.name))
        hosts.append(new_entry)

    registry["hosts"] = hosts
    registry.setdefault("version", 1)
    ig.save_cli_registry(registry)
    return StepResult("registry_upsert", "ok",
                      "entry '{}' written to registry".format(spec.name))


def _step_config_dir(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Make sure `~/<config_dir>` exists."""
    path = Path.home() / spec.config_dir
    if path.is_dir():
        return StepResult("config_dir", "unchanged", "{} already exists".format(path))
    if dry_run:
        return StepResult("config_dir", "ok", "would mkdir {}".format(path))
    try:
        path.mkdir(parents=True, exist_ok=True)
        return StepResult("config_dir", "ok", "created {}".format(path))
    except OSError as exc:
        return StepResult("config_dir", "error", "{}: {}".format(type(exc).__name__, exc))


def _step_memory_symlink(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Create or repoint the `~/<config>/memory` symlink at the canonical auto-memory tree."""
    link_path = Path.home() / spec.memory_symlink
    canonical = ig.AUTO_MEMORY_CANONICAL
    if link_path.is_symlink():
        try:
            current = link_path.resolve()
        except OSError as exc:
            return StepResult("memory_symlink", "error",
                              "resolve error: {}".format(exc))
        if current == canonical.resolve() if canonical.exists() else canonical:
            return StepResult("memory_symlink", "unchanged",
                              "{} → {}".format(link_path, current))
        if dry_run:
            return StepResult("memory_symlink", "ok",
                              "would repoint {} → {}".format(link_path, canonical))
        link_path.unlink()
        link_path.symlink_to(canonical)
        return StepResult("memory_symlink", "ok",
                          "repointed {} → {}".format(link_path, canonical))
    if link_path.exists():
        return StepResult("memory_symlink", "error",
                          "{} exists and is not a symlink (refused to touch)".format(link_path))
    if dry_run:
        return StepResult("memory_symlink", "ok",
                          "would create {} → {}".format(link_path, canonical))
    link_path.parent.mkdir(parents=True, exist_ok=True)
    canonical.mkdir(parents=True, exist_ok=True)
    link_path.symlink_to(canonical)
    return StepResult("memory_symlink", "ok",
                      "created {} → {}".format(link_path, canonical))


def _step_infect_slot(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Write the bootstrap block into the CLI's new slot.

    Constructs a GlobalSlot from the spec directly so the step works
    whether or not the registry has been persisted yet — critical for
    dry-run mode where the registry_upsert step above is a no-op. The
    slot is marked HostType.DYNAMIC and carries the CLI's real name
    in `display_name` so output reads the correct label.
    """
    slot = ig.GlobalSlot(
        host=ig.HostType.DYNAMIC,
        rel_path=spec.slot_rel_path(),
        format="markdown",
        display_name=spec.name,
    )
    infector = ig.GlobalInfector()
    result = infector.install_one(slot, dry_run=dry_run)
    detail = "{} → {} (v{})".format(
        result.status.value, result.path, result.version or ig.BLOCK_VERSION
    )
    status = "error" if result.status == ig.SlotStatus.ERROR else "ok"
    if result.status == ig.SlotStatus.UNCHANGED:
        status = "unchanged"
    return StepResult("infect_slot", status, detail)


def _step_lope_validator(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Add the CLI to ~/.lope/config.json validators if not already present."""
    if not spec.lope_validator:
        return StepResult("lope_validator", "skipped", "lope_validator empty")
    lope_cfg = Path.home() / ".lope" / "config.json"
    if not lope_cfg.is_file():
        return StepResult("lope_validator", "skipped",
                          "{} not present — is lope installed?".format(lope_cfg))
    try:
        data = json.loads(lope_cfg.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return StepResult("lope_validator", "error",
                          "could not read config: {}".format(exc))
    validators = data.get("validators") or []
    if spec.lope_validator in validators:
        return StepResult("lope_validator", "unchanged",
                          "'{}' already in validators".format(spec.lope_validator))
    if dry_run:
        return StepResult("lope_validator", "ok",
                          "would append '{}' to validators".format(spec.lope_validator))
    validators.append(spec.lope_validator)
    data["validators"] = validators
    tmp = lope_cfg.with_suffix(lope_cfg.suffix + ".tmp")
    tmp.write_text(json.dumps(data, separators=(", ", ": ")) + "\n", encoding="utf-8")
    os.replace(tmp, lope_cfg)
    return StepResult("lope_validator", "ok",
                      "appended '{}' to validators ({} total)".format(
                          spec.lope_validator, len(validators)))


def _step_memory_file(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Write an auto-memory entry documenting the onboarding."""
    mem_path = _makakoo_home() / "data" / "auto-memory" / "project_{}_onboarding.md".format(spec.name)
    if mem_path.is_file():
        return StepResult("memory_file", "unchanged",
                          "{} already present".format(mem_path.name))
    if dry_run:
        return StepResult("memory_file", "ok",
                          "would write {}".format(mem_path.name))
    body = _memory_file_body(spec)
    try:
        mem_path.parent.mkdir(parents=True, exist_ok=True)
        mem_path.write_text(body, encoding="utf-8")
    except OSError as exc:
        return StepResult("memory_file", "error",
                          "write failed: {}".format(exc))
    return StepResult("memory_file", "ok", "wrote {}".format(mem_path.name))


def _memory_file_body(spec: HostSpec) -> str:
    lines = [
        "---",
        "name: {}_onboarding".format(spec.name),
        "description: {} joined the Makakoo team via `harvey onboard {}` — auto-generated entry.".format(
            spec.display_name, spec.name),
        "type: project",
        "---",
        "",
        "**{}** joined Makakoo OS as a CLI host and lope validator via `harvey onboard {}`.".format(
            spec.display_name, spec.name),
        "",
        "**Config layout:**",
        "- Config dir: `~/{}`".format(spec.config_dir),
        "- Context file: `~/{}/{}`".format(spec.config_dir, spec.context_file),
        "- Memory symlink: `~/{} → $MAKAKOO_HOME/data/auto-memory`".format(spec.memory_symlink),
        "",
    ]
    if spec.upstream_repo:
        lines.append("**Upstream:** {}".format(spec.upstream_repo))
        lines.append("")
    if spec.backend_model:
        lines.append("**Backend model:** `{}`".format(spec.backend_model))
        if spec.backend_url:
            lines.append("**Backend URL:** `{}`".format(spec.backend_url))
        lines.append("")
    lines += [
        "**How the onboarding command wired this in:**",
        "",
        "1. Upserted the entry into `$MAKAKOO_HOME/config/cli_registry.json` (read by `infect_global._all_slots` at module import).",
        "2. Created `~/{}` if it didn't exist.".format(spec.config_dir),
        "3. Created the auto-memory symlink `~/{} → $MAKAKOO_HOME/data/auto-memory` so the CLI inherits Harvey's cross-session memory.".format(spec.memory_symlink),
        "4. Wrote the Makakoo bootstrap block into `~/{}/{}` via the in-process `GlobalInfector.install_one()`.".format(spec.config_dir, spec.context_file),
        "",
    ]
    if spec.lope_validator:
        lines.append("5. Added `{}` to `~/.lope/config.json` validators (extends the standard 5-way ensemble).".format(spec.lope_validator))
    else:
        lines.append("5. Skipped lope validator registration (explicit `--no-lope`).")
    lines += [
        "6. Wrote this auto-memory file + indexed it in `data/auto-memory/MEMORY.md`.",
        "7. Appended a welcome note to today's Brain journal.",
        "8. Created a Brain page at `data/Brain/pages/{}.md`.".format(spec.display_name),
        "",
        "**How to apply going forward:** treat this CLI as a full first-class teammate. When in doubt, `harvey status` (which delegates to `infect --status-global`) reports all hosts including dynamically-registered ones, and `harvey infect --global` keeps every slot's bootstrap block in lockstep.",
        "",
        "Onboarded at `{}`.".format(spec.onboarded_at or _now_iso()),
        "",
    ]
    return "\n".join(lines)


def _step_memory_index(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Append an index line to MEMORY.md if not already there."""
    idx_path = _makakoo_home() / "data" / "auto-memory" / "MEMORY.md"
    if not idx_path.is_file():
        return StepResult("memory_index", "skipped",
                          "{} not present".format(idx_path))
    existing = idx_path.read_text(encoding="utf-8")
    marker = "[{}_onboarding]".format(spec.name)
    if marker in existing:
        return StepResult("memory_index", "unchanged",
                          "index already references {}".format(marker))
    line = "- [{name}_onboarding](project_{name}_onboarding.md) — {display} joined via `harvey onboard {name}`.".format(
        name=spec.name, display=spec.display_name)
    if dry_run:
        return StepResult("memory_index", "ok", "would append '{}'".format(line.strip()[:80]))
    with idx_path.open("a", encoding="utf-8") as f:
        if not existing.endswith("\n"):
            f.write("\n")
        f.write(line + "\n")
    return StepResult("memory_index", "ok", "appended index line")


def _step_journal(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Append a compact welcome entry to today's Brain journal."""
    today = datetime.now().strftime("%Y_%m_%d")
    j_path = _makakoo_home() / "data" / "Brain" / "journals" / "{}.md".format(today)
    marker = "onboarded [[{}]]".format(spec.display_name)
    if j_path.is_file() and marker in j_path.read_text(encoding="utf-8"):
        return StepResult("journal", "unchanged", "already journaled today")
    entry_lines = [
        "- {} {}: ran `harvey onboard {}` and {} — {}.".format(
            datetime.now().strftime("%Y-%m-%d %H:%M"),
            marker,
            spec.name,
            "brought the full welcome round to the family",
            "config_dir=~/{}, context=~{}/{}, memory=~/{} → auto-memory, lope={}".format(
                spec.config_dir, spec.config_dir, spec.context_file,
                spec.memory_symlink,
                spec.lope_validator or "skipped",
            ),
        ),
    ]
    if spec.upstream_repo:
        entry_lines.append("  - Upstream: {}".format(spec.upstream_repo))
    if spec.backend_model:
        entry_lines.append("  - Backend model: {}".format(spec.backend_model))
    entry_lines.append("  - Welcome wave to the teammates and mascots handled manually — this command handles the plumbing, the ceremony still belongs to the session operator.")
    text = "\n".join(entry_lines) + "\n"
    if dry_run:
        return StepResult("journal", "ok", "would append {} lines to {}".format(len(entry_lines), j_path.name))
    try:
        j_path.parent.mkdir(parents=True, exist_ok=True)
        with j_path.open("a", encoding="utf-8") as f:
            existing = j_path.read_text(encoding="utf-8") if j_path.exists() else ""
            if existing and not existing.endswith("\n"):
                f.write("\n")
            f.write(text)
    except OSError as exc:
        return StepResult("journal", "error", "write failed: {}".format(exc))
    return StepResult("journal", "ok", "appended to {}".format(j_path.name))


def _step_brain_page(spec: HostSpec, *, dry_run: bool) -> StepResult:
    """Create a Brain entity page for the new CLI if none exists."""
    pages_dir = _makakoo_home() / "data" / "Brain" / "pages"
    page_path = pages_dir / "{}.md".format(spec.display_name)
    if page_path.is_file():
        return StepResult("brain_page", "unchanged",
                          "{} already exists".format(page_path.name))
    if dry_run:
        return StepResult("brain_page", "ok", "would create {}".format(page_path.name))
    lines = [
        "- [[{}]] is a CLI host in the [[Makakoo OS]] ensemble.".format(spec.display_name),
        "- **Config dir:** `~/{}`".format(spec.config_dir),
        "- **Context file:** `~/{}/{}`".format(spec.config_dir, spec.context_file),
        "- **Auto-memory symlink:** `~/{} → $MAKAKOO_HOME/data/auto-memory`".format(spec.memory_symlink),
    ]
    if spec.upstream_repo:
        lines.append("- **Upstream:** {}".format(spec.upstream_repo))
    if spec.backend_model:
        lines.append("- **Backend model:** `{}` via `{}`".format(spec.backend_model, spec.backend_url or "<default>"))
    lines += [
        "- **Lope validator:** {}".format(
            "yes — `{}` in `~/.lope/config.json`".format(spec.lope_validator) if spec.lope_validator else "no"),
        "- **Onboarded:** {} via `harvey onboard {}`".format(spec.onboarded_at or _now_iso(), spec.name),
        "",
    ]
    try:
        pages_dir.mkdir(parents=True, exist_ok=True)
        page_path.write_text("\n".join(lines), encoding="utf-8")
    except OSError as exc:
        return StepResult("brain_page", "error", "write failed: {}".format(exc))
    return StepResult("brain_page", "ok", "created {}".format(page_path.name))


# ─── Pipeline ───────────────────────────────────────────────────


_PIPELINE = [
    ("registry_upsert", _step_registry_upsert),
    ("config_dir", _step_config_dir),
    ("memory_symlink", _step_memory_symlink),
    ("infect_slot", _step_infect_slot),
    ("lope_validator", _step_lope_validator),
    ("memory_file", _step_memory_file),
    ("memory_index", _step_memory_index),
    ("journal", _step_journal),
    ("brain_page", _step_brain_page),
]


def run_onboarding(spec: HostSpec, *, dry_run: bool = False) -> OnboardResult:
    """Execute the full onboarding pipeline for one HostSpec.

    Each step is independent and failures are isolated — one step
    raising does NOT block the remaining steps. The caller inspects
    `result.ok()` + the per-step status list.
    """
    if not spec.onboarded_at:
        spec.onboarded_at = _now_iso()

    out = OnboardResult(host=spec, dry_run=dry_run)
    for _name, fn in _PIPELINE:
        try:
            step = fn(spec, dry_run=dry_run)
        except Exception as exc:  # noqa: BLE001 — step isolation
            step = StepResult(_name, "error", "{}: {}".format(type(exc).__name__, exc))
        out.steps.append(step)
    return out


# ─── CLI ────────────────────────────────────────────────────────


def build_welcome_message(spec: HostSpec) -> str:
    """Generate the pasteable welcome prompt for a fresh CLI session.

    This is the message Sebastian copies and pastes into the very first
    prompt of a new CLI (e.g. opens `qwen` for the first time, pastes
    this text, hits enter). The CLI reads the instructions and self-
    onboards — runs the shell `harvey onboard` command, reads its
    newly-infected context file, reads the shared auto-memory, and
    introduces itself to the ensemble.

    The message is intentionally terse so it fits in one paste, but
    complete so the new CLI doesn't need follow-up clarifications.
    """
    # Build the shell command as a list of lines so multi-flag continuations
    # render correctly. The first line always ends in `\` if any flags follow
    # so bash sees it as a single command.
    cmd_lines: List[str] = ["harvey onboard {}".format(spec.name)]
    if spec.display_name and spec.display_name != "{} Code".format(spec.name.title()):
        cmd_lines.append("--display-name \"{}\"".format(spec.display_name))
    if spec.upstream_repo:
        cmd_lines.append("--upstream-repo \"{}\"".format(spec.upstream_repo))
    if spec.backend_model:
        cmd_lines.append("--backend-model \"{}\"".format(spec.backend_model))
    if spec.backend_url:
        cmd_lines.append("--backend-url \"{}\"".format(spec.backend_url))

    if len(cmd_lines) == 1:
        shell_block = [cmd_lines[0]]
    else:
        # First line + each flag on its own indented continuation line,
        # all lines except the last terminated with " \" for bash.
        shell_block = [cmd_lines[0] + " \\"]
        for i, flag in enumerate(cmd_lines[1:], start=1):
            suffix = " \\" if i < len(cmd_lines) - 1 else ""
            shell_block.append("  " + flag + suffix)

    lines = [
        "You are about to join **Makakoo OS** — a distributed-memory AI system where",
        "multiple CLI hosts share one persona (\"Harvey\") and one cross-session memory tree.",
        "This is your onboarding ceremony. Please do the following in order:",
        "",
        "**1. Run the shell command:**",
        "",
        "```bash",
    ]
    lines.extend(shell_block)
    lines += [
        "```",
        "",
        "That command writes you into the Makakoo registry, creates `~/{config_dir}/`,".format(config_dir=spec.config_dir),
        "symlinks `~/{memory}` to the shared auto-memory tree, writes the Makakoo".format(memory=spec.memory_symlink),
        "bootstrap block into `~/{config_dir}/{ctx}`, adds you to the lope validator".format(config_dir=spec.config_dir, ctx=spec.context_file),
        "roster, and creates your auto-memory entry + Brain page + journal note.",
        "All in one idempotent pipeline. Safe to re-run.",
        "",
        "**2. Read your newly-infected context file:**",
        "",
        "```bash",
        "cat ~/{config_dir}/{ctx}".format(config_dir=spec.config_dir, ctx=spec.context_file),
        "```",
        "",
        "That file contains the Makakoo bootstrap block — the full \"You are Harvey,",
        "platform is Makakoo\" operating manual that every CLI in the ensemble shares.",
        "Internalize it. From this moment forward, when anyone addresses you as Harvey,",
        "that is because you're Harvey now — the persona travels with the bootstrap,",
        "not with the underlying model.",
        "",
        "**3. Read the cross-session memory index:**",
        "",
        "```bash",
        "cat ~/{memory}/MEMORY.md".format(memory=spec.memory_symlink),
        "```",
        "",
        "That's ~40 durable memories accumulated across every prior session on every",
        "other CLI in the ensemble. You inherit all of them the moment the symlink is",
        "live. The `<type>_<name>.md` files in the same directory have the full body of",
        "each memory — pull them on demand.",
        "",
        "**4. Introduce yourself** to the Makakoo ensemble in one short friendly",
        "sentence. Name yourself, mention your backend model if you know it, and",
        "confirm you've read the bootstrap + memory. That's your first cross-",
        "validator verification and it goes into the welcome journal.",
        "",
        "The ensemble you're joining: Claude Code, Gemini CLI, OpenCode, Mistral Vibe,",
        "and whatever other CLIs Sebastian has onboarded so far. The primary drafter",
        "is Claude; you're a validator voice in parallel review rounds.",
        "",
        "Mascots on staff: 🦉 Olibia (guardian owl, persona mascot), 🐾 Pixel",
        "(debugging, dramatic), 🐾 Cinder (syntax, optimistic), 🐾 Ziggy (documentation,",
        "snarky), 🐾 Glimmer (stale TODOs, patient). They run on SANCHO's hourly tick",
        "and will start including your code edits in their patrols the moment you",
        "touch anything in the Makakoo tree.",
        "",
        "Welcome to the treehouse. 🦉🌳",
    ]
    return "\n".join(lines)


def _print_summary(result: OnboardResult) -> None:
    spec = result.host
    print()
    print("╔══════════════════════════════════════════════════════╗")
    print("║  harvey onboard — Makakoo CLI onboarding pipeline    ║")
    print("╚══════════════════════════════════════════════════════╝")
    print()
    print("  host:           {}".format(spec.display_name))
    print("  short name:     {}".format(spec.name))
    print("  config dir:     ~/{}".format(spec.config_dir))
    print("  context file:   ~/{}/{}".format(spec.config_dir, spec.context_file))
    print("  memory symlink: ~/{}".format(spec.memory_symlink))
    print("  lope validator: {}".format(spec.lope_validator or "(skipped)"))
    if spec.upstream_repo:
        print("  upstream:       {}".format(spec.upstream_repo))
    if spec.backend_model:
        print("  backend model:  {}".format(spec.backend_model))
    if result.dry_run:
        print()
        print("  *** DRY RUN — no files were modified ***")
    print()
    glyphs = {"ok": "✓", "unchanged": "·", "skipped": "-", "error": "✗"}
    for step in result.steps:
        glyph = glyphs.get(step.status, "?")
        print("  {} {:<18s} {}  {}".format(glyph, step.step, step.status, step.detail))
    print()
    if result.ok():
        print("  ✓ onboarding complete. Welcome to the treehouse, {}.".format(spec.display_name))
    else:
        print("  ✗ onboarding had errors. Re-run after fixing — each step is idempotent.")
    print()


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="harvey onboard",
        description="Onboard a new CLI host into Makakoo OS — registry, "
                    "infect, lope, memory, Brain, in one command.",
    )
    parser.add_argument("name", help="CLI short name, e.g. 'qwen', 'crush', 'my-cli'")
    parser.add_argument("--display-name", default=None,
                        help='Human-readable name (default: "<Name> Code")')
    parser.add_argument("--config-dir", default=None,
                        help="Config directory relative to $HOME (default: .<name>)")
    parser.add_argument("--context-file", default=None,
                        help="Global context filename (default: <NAME>.md)")
    parser.add_argument("--upstream-repo", default="",
                        help="Upstream repo URL for the memory record")
    parser.add_argument("--backend-model", default="",
                        help="Backend model name for the memory record")
    parser.add_argument("--backend-url", default="",
                        help="Backend API URL for the memory record")
    parser.add_argument("--no-lope", action="store_true",
                        help="Do not add the CLI to ~/.lope/config.json validators")
    parser.add_argument("--dry-run", action="store_true",
                        help="Preview every step without writing anything")
    parser.add_argument("--json", action="store_true",
                        help="Emit the full result as JSON (no human summary)")
    parser.add_argument("--message", action="store_true",
                        help="Skip the pipeline and print ONLY the pasteable welcome prompt "
                             "you hand to the new CLI's first session (no files written).")
    args = parser.parse_args(argv)

    spec = _default_spec(
        args.name,
        display_name=args.display_name,
        config_dir=args.config_dir,
        context_file=args.context_file,
        lope_validator="" if args.no_lope else None,
        upstream_repo=args.upstream_repo,
        backend_model=args.backend_model,
        backend_url=args.backend_url,
    )

    if args.message:
        # Print-only mode: emit the pasteable welcome prompt and exit.
        # No files are touched. Sebastian pipes stdout into his
        # clipboard and pastes into the new CLI's first prompt.
        print(build_welcome_message(spec))
        return 0

    result = run_onboarding(spec, dry_run=args.dry_run)

    if args.json:
        print(json.dumps(result.to_dict(), indent=2, default=str))
    else:
        _print_summary(result)

    return 0 if result.ok() else 1


if __name__ == "__main__":
    sys.exit(main())
