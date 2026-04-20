"""
AgentRegistry — discover and index all Harvey plugin agents.

Scans two sources:
  1. External agents: `$HARVEY_HOME/agents/<name>/agent.yaml`
  2. Internal subagents: class-reflection over `core/subagents/*_agent.py`

Both feed into a single `list_all()` view. Callers like the planner and
the harvey_agent `spawn_subagent` tool (Phase 6) consume this view to
discover available agents at runtime, without hard-coding a list.

The registry is read-mostly — callers instantiate once, then call
`get(name)` / `list_all()` / `list_by_type()` freely. `reload()` re-scans
both sources from disk for long-lived daemons.
"""

from __future__ import annotations

import importlib
import inspect
import logging
import os
import pkgutil
from pathlib import Path
from typing import Dict, List, Optional

from .manifest import (
    AgentManifest,
    AgentState,
    AgentType,
    ManifestValidationError,
)

log = logging.getLogger("harvey.agents.loader")

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
DEFAULT_AGENTS_DIR = os.path.join(HARVEY_HOME, "agents")
DEFAULT_SUBAGENTS_PACKAGE = "core.subagents"


class AgentRegistry:
    """Central index of all Harvey plugin agents."""

    def __init__(
        self,
        agents_dir: Optional[str] = None,
        subagents_package: Optional[str] = None,
        auto_load: bool = True,
    ):
        self.agents_dir = agents_dir or DEFAULT_AGENTS_DIR
        self.subagents_package = subagents_package or DEFAULT_SUBAGENTS_PACKAGE
        self._manifests: Dict[str, AgentManifest] = {}
        self._errors: List[str] = []
        if auto_load:
            self.reload()

    # ─── Loading ────────────────────────────────────────────────

    def reload(self) -> Dict[str, AgentManifest]:
        """Re-scan both sources from disk. Returns the current index."""
        self._manifests.clear()
        self._errors.clear()
        self._load_yaml_agents()
        self._load_subagent_classes()
        return dict(self._manifests)

    def _load_yaml_agents(self) -> None:
        """Scan `agents/<name>/agent.yaml` and build manifests."""
        if not os.path.isdir(self.agents_dir):
            log.debug(f"[registry] agents dir not found: {self.agents_dir}")
            return

        for entry in sorted(os.listdir(self.agents_dir)):
            agent_path = os.path.join(self.agents_dir, entry)
            if not os.path.isdir(agent_path) or entry.startswith("."):
                continue
            yaml_path = os.path.join(agent_path, "agent.yaml")
            if not os.path.isfile(yaml_path):
                log.debug(f"[registry] {entry}: no agent.yaml, skipping")
                continue
            try:
                with open(yaml_path, "r") as f:
                    text = f.read()
                manifest = AgentManifest.from_yaml_text(
                    text,
                    source_path=os.path.relpath(yaml_path, HARVEY_HOME),
                )
                self._add(manifest)
                log.info(
                    f"[registry] loaded YAML manifest: {manifest.name} "
                    f"(type={manifest.type.value}, status={manifest.status.value})"
                )
            except ManifestValidationError as e:
                msg = f"[registry] {entry}/agent.yaml invalid: {e}"
                log.warning(msg)
                self._errors.append(msg)
            except Exception as e:
                msg = f"[registry] {entry}/agent.yaml load error: {e}"
                log.error(msg, exc_info=True)
                self._errors.append(msg)

    def _load_subagent_classes(self) -> None:
        """Discover Subagent subclasses in core.subagents.* and synthesize manifests."""
        try:
            from core.subagents.subagent import Subagent
        except Exception as e:
            log.warning(f"[registry] cannot import Subagent base class: {e}")
            return

        # Walk every module in the package
        try:
            package = importlib.import_module(self.subagents_package)
        except Exception as e:
            log.warning(f"[registry] cannot import package {self.subagents_package}: {e}")
            return

        pkg_path = getattr(package, "__path__", None)
        if not pkg_path:
            return

        for mod_info in pkgutil.iter_modules(pkg_path):
            full_name = f"{self.subagents_package}.{mod_info.name}"
            if mod_info.name.startswith("_") or mod_info.name == "subagent":
                continue  # skip private modules and the base class file
            try:
                mod = importlib.import_module(full_name)
            except Exception as e:
                log.warning(f"[registry] import failed for {full_name}: {e}")
                self._errors.append(f"subagent import failed: {full_name}: {e}")
                continue

            for name, obj in inspect.getmembers(mod, inspect.isclass):
                if obj is Subagent:
                    continue
                if not issubclass(obj, Subagent):
                    continue
                if obj.__module__ != full_name:
                    continue  # skip re-exports
                try:
                    manifest = AgentManifest.from_subagent_class(
                        obj, source_path=f"{self.subagents_package}.{mod_info.name}"
                    )
                    # Avoid shadowing an explicit YAML manifest for the same name
                    if manifest.name in self._manifests:
                        existing = self._manifests[manifest.name]
                        log.debug(
                            f"[registry] subagent {manifest.name} already registered "
                            f"via {existing.source_path} — keeping YAML source"
                        )
                        continue
                    self._add(manifest)
                    log.info(
                        f"[registry] discovered subagent: {manifest.name} "
                        f"({len(manifest.tools)} action(s))"
                    )
                except Exception as e:
                    msg = f"[registry] {full_name}.{name} manifest synthesis failed: {e}"
                    log.warning(msg)
                    self._errors.append(msg)

    def _add(self, manifest: AgentManifest) -> None:
        if manifest.name in self._manifests:
            existing = self._manifests[manifest.name]
            log.warning(
                f"[registry] duplicate agent name '{manifest.name}' "
                f"(existing: {existing.source_path}, new: {manifest.source_path}) "
                f"— first registration wins"
            )
            return
        self._manifests[manifest.name] = manifest

    # ─── Query API ──────────────────────────────────────────────

    def get(self, name: str) -> Optional[AgentManifest]:
        return self._manifests.get(name)

    def list_all(self) -> List[AgentManifest]:
        return sorted(self._manifests.values(), key=lambda m: m.name)

    def list_by_type(self, agent_type: AgentType) -> List[AgentManifest]:
        return sorted(
            (m for m in self._manifests.values() if m.type == agent_type),
            key=lambda m: m.name,
        )

    def list_active(self) -> List[AgentManifest]:
        return sorted(
            (m for m in self._manifests.values() if m.is_active()),
            key=lambda m: m.name,
        )

    def names(self) -> List[str]:
        return sorted(self._manifests.keys())

    def count(self) -> int:
        return len(self._manifests)

    def errors(self) -> List[str]:
        """Load errors from the last reload() — for diagnostics."""
        return list(self._errors)
