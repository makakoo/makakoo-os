"""
Harvey Subagent Framework (Phase 2).

In-process specialized agents that coordinate through the ArtifactStore
and PersistentEventBus. Each subagent is a step handler for the
AsyncDAGExecutor.

NOT to be confused with core/agents/ which is a standalone agent
scaffolding tool (unrelated — creates agents/weather-monitor/ etc.).
"""

from core.subagents.subagent import Subagent, DEFAULT_TOOLS
from core.subagents.image_gen_agent import ImageGenAgent
from core.subagents.researcher_agent import ResearcherAgent
from core.subagents.synthesizer_agent import SynthesizerAgent
from core.subagents.storage_agent import StorageAgent
from core.subagents.task_master_agent import TaskMasterAgent
from core.subagents.olibia_agent import OlibiaAgent

__all__ = [
    "Subagent",
    "DEFAULT_TOOLS",
    "ImageGenAgent",
    "ResearcherAgent",
    "SynthesizerAgent",
    "StorageAgent",
    "TaskMasterAgent",
    "OlibiaAgent",
]
