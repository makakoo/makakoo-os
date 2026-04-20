"""Harvey OS Core — Registry infrastructure.

Provides:
- SkillRegistry: discovers and matches skills via SKILL.md files
- ToolRegistry: dispatches runtime tools with self-registration

Usage:
    from harvey_os.core.registry import registry, SkillRegistry, ToolRegistry
"""

# Tool Registry (runtime tool dispatch with self-registration)
from .tool_registry import registry, ToolEntry, ToolRegistry

# Re-export for convenience
__all__ = ["registry", "ToolEntry", "ToolRegistry"]
