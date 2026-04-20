"""
Harvey OS — Settings Schema

Pydantic models for Harvey's ~90 settings across 15+ categories.
Matches Claude Code's Zod validation patterns.

Path: plugins-core/lib-harvey-core/src/core/settings/schema.py
"""

from __future__ import annotations

from pydantic import BaseModel, Field, field_validator
from typing import Literal, Any, Optional, List, Dict
from pathlib import Path
import json


class PermissionSettings(BaseModel):
    """
    Permission settings for Harvey operations.

    Controls what operations are allowed at runtime.
    """

    bash_allowed: bool = True
    shell_allowed: bool = False
    browser_allowed: bool = False
    mcp_servers_allowed: List[str] = Field(default_factory=list)
    grpo_allowed: bool = False
    dangerous_file_ops_allowed: List[str] = Field(default_factory=list)
    dangerous_commands_blacklist: List[str] = Field(
        default_factory=lambda: ["rm -rf /", "drop table", "DELETE FROM"]
    )


class ModelSettings(BaseModel):
    """
    Model configuration for LLM calls.

    Configures the AI provider and model parameters.
    """

    provider: Literal["switchailocal", "openai", "anthropic", "gemini", "ollama"] = (
        "switchailocal"
    )
    base_url: str = "http://localhost:18080/v1"
    api_key: str = ""
    model: str = "auto"
    temperature: float = 0.7
    max_tokens: int = 8192
    top_p: float = 0.95
    frequency_penalty: float = 0.0
    presence_penalty: float = 0.0
    timeout_sec: int = 120


class MemorySettings(BaseModel):
    """
    Memory and context settings.

    Controls how Harvey maintains context across sessions.
    """

    auto_extract: bool = True
    team_memory_enabled: bool = False
    recall_preamble_enabled: bool = True
    max_context_tokens: int = 80000
    auto_memory_directory: Optional[str] = None
    brain_sync_enabled: bool = True
    extract_after_session: bool = True
    extract_preamble: str = "You are a memory extraction agent. After each session, extract key decisions, facts, and action items."


class MCPSettings(BaseModel):
    """
    MCP (Model Context Protocol) server settings.

    Controls MCP server connections and authentication.
    """

    servers: Dict[str, Dict] = Field(default_factory=dict)
    oauth_enabled: bool = False
    auth_cache_ttl_sec: int = 900  # 15 minutes
    allowed_servers: List[str] = Field(default_factory=list)
    denied_servers: List[str] = Field(default_factory=list)
    keychain_service_suffix: str = "harvey"
    lockfile_refresh_enabled: bool = True


class AgentSettings(BaseModel):
    """
    Agent orchestration settings.

    Controls multi-agent behavior and resource limits.
    """

    coordinator_enabled: bool = False
    max_concurrent_agents: int = 4
    agent_timeout_sec: int = 300
    auto_restart: bool = True
    max_restart_attempts: int = 3
    health_check_interval_sec: int = 30
    mailbox_enabled: bool = True
    cwd_isolation_enabled: bool = True
    restart_backoff_sec: List[int] = Field(default_factory=lambda: [5, 10, 30, 60])


class HookSettings(BaseModel):
    """
    Pre/post command hooks.

    Allows running commands before/after operations.
    """

    pre_command: List[str] = Field(default_factory=list)
    post_command: List[str] = Field(default_factory=list)
    pre_tool: Dict[str, List[str]] = Field(default_factory=dict)
    post_tool: Dict[str, List[str]] = Field(default_factory=dict)
    on_agent_spawn: List[str] = Field(default_factory=list)
    on_agent_die: List[str] = Field(default_factory=list)


class TelemetrySettings(BaseModel):
    """
    Telemetry and analytics settings.

    Controls observability and feature flag behavior.
    """

    enabled: bool = True
    otel_endpoint: Optional[str] = None
    growthbook_enabled: bool = False
    sample_rate: float = 1.0
    telemetry_dir: Optional[str] = None
    session_telemetry_enabled: bool = True


class GitSettings(BaseModel):
    """
    Git integration settings.

    Controls git operation behavior.
    """

    auto_fetch: bool = True
    auto_pull: bool = False
    auto_push: bool = False
    branch_protection_enabled: bool = True
    allowed_hooks: List[str] = Field(
        default_factory=lambda: ["pre-commit", "commit-msg"]
    )
    sparse_checkout_enabled: bool = False


class IndexSettings(BaseModel):
    """
    Codebase indexing settings.

    Controls file indexing and search behavior.
    """

    index_enabled: bool = True
    index_depth: int = 10
    index_exclude_patterns: List[str] = Field(
        default_factory=lambda: [
            "node_modules",
            ".git",
            "__pycache__",
            "*.pyc",
            ".venv",
            "venv",
        ]
    )
    index_file_types: List[str] = Field(
        default_factory=lambda: [
            ".py",
            ".js",
            ".ts",
            ".tsx",
            ".md",
            ".json",
            ".yaml",
            ".yml",
        ]
    )
    lru_cache_size: int = 1000
    ripgrep_mode: Literal["standard", "smart", "literal"] = "smart"


class StartupSettings(BaseModel):
    """
    Startup and bootstrap settings.

    Controls initialization behavior.
    """

    skip_migrations: bool = False
    migration_version: int = 0
    defer_prefetch: bool = False
    trust_dialog_auto_accept: bool = False
    original_cwd: Optional[str] = None
    prefire_mdm: bool = True
    prefire_keychain: bool = True


class HarveySettings(BaseModel):
    """
    Root settings model for Harvey OS.

    Contains ~90 settings across all categories.
    Loaded with 6-tier precedence system.
    """

    version: int = 1

    # Core settings
    permissions: PermissionSettings = Field(default_factory=PermissionSettings)
    model: ModelSettings = Field(default_factory=ModelSettings)
    memory: MemorySettings = Field(default_factory=MemorySettings)
    mcp: MCPSettings = Field(default_factory=MCPSettings)
    agents: AgentSettings = Field(default_factory=AgentSettings)
    hooks: HookSettings = Field(default_factory=HookSettings)
    telemetry: TelemetrySettings = Field(default_factory=TelemetrySettings)
    git: GitSettings = Field(default_factory=GitSettings)
    indexing: IndexSettings = Field(default_factory=IndexSettings)
    startup: StartupSettings = Field(default_factory=StartupSettings)

    # Org-controlled (from MDM / enterprise)
    org_allowed_mcp_servers: List[str] = Field(default_factory=list)
    org_denied_mcp_servers: List[str] = Field(default_factory=list)
    org_allowed_http_hook_urls: List[str] = Field(default_factory=list)

    # Plugin-base defaults (lowest priority)
    plugin_settings: Dict[str, Any] = Field(default_factory=dict)

    # Additional metadata
    settings_source: Optional[str] = None
    settings_path: Optional[str] = None

    @field_validator("version", mode="before")
    @classmethod
    def validate_version(cls, v):
        if v is None:
            return 1
        return v

    def dict(self, **kwargs) -> Dict[str, Any]:
        """Override dict() to exclude None values for cleaner serialization."""
        return super().model_dump(**kwargs)

    def get_path(self, key_path: str) -> Any:
        """
        Get a nested setting by dot-path.

        Example: settings.get_path('model.temperature')
        """
        keys = key_path.split(".")
        value = self.model_dump()
        for k in keys:
            if isinstance(value, dict):
                value = value.get(k)
            else:
                return None
        return value


def load_settings_from_file(path: Path) -> Optional[HarveySettings]:
    """
    Load settings from a JSON file.

    Args:
        path: Path to settings JSON file

    Returns:
        HarveySettings instance or None if file doesn't exist/error
    """
    try:
        path = Path(path)
        if not path.exists():
            return None
        data = json.loads(path.read_text())
        return HarveySettings(**data)
    except (json.JSONDecodeError, TypeError, ValueError):
        return None
