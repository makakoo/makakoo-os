"""
HarveyChat configuration — loads from data/chat/config.json with env overrides.
"""

import json
import logging
import os
from dataclasses import dataclass, field
from pathlib import Path

log = logging.getLogger("harveychat.config")

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
CHAT_DATA_DIR = Path(HARVEY_HOME) / "data" / "chat"
CONFIG_PATH = CHAT_DATA_DIR / "config.json"


@dataclass
class TelegramConfig:
    bot_token: str = ""
    allowed_user_ids: list = field(default_factory=list)   # empty = allow all
    allowed_chat_ids: list = field(default_factory=list)   # groups + channels (-100…)
    polling_timeout: int = 30
    ignore_bots: bool = True  # silently drop messages from other bots


@dataclass
class BridgeConfig:
    """How HarveyChat reaches Harvey's brain."""

    # Primary: switchAILocal (same gateway all agents use)
    switchai_url: str = "http://localhost:18080/v1"
    switchai_model: str = "auto"
    switchai_api_key: str = ""
    # Fallback: direct Anthropic API
    anthropic_api_key: str = ""
    anthropic_model: str = "claude-sonnet-4-20250514"
    # System prompt context
    max_history_messages: int = 20
    max_tokens: int = 4096


@dataclass
class ChatConfig:
    telegram: TelegramConfig = field(default_factory=TelegramConfig)
    bridge: BridgeConfig = field(default_factory=BridgeConfig)
    db_path: str = ""
    log_to_brain: bool = True
    pid_file: str = ""

    def __post_init__(self):
        if not self.db_path:
            self.db_path = str(CHAT_DATA_DIR / "conversations.db")
        if not self.pid_file:
            self.pid_file = str(CHAT_DATA_DIR / "harveychat.pid")


def load_config() -> ChatConfig:
    """Load config from file, with env var overrides."""
    cfg = ChatConfig()

    # Load from file if exists
    if CONFIG_PATH.exists():
        try:
            raw = json.loads(CONFIG_PATH.read_text())
            if "telegram" in raw:
                for k, v in raw["telegram"].items():
                    if hasattr(cfg.telegram, k):
                        setattr(cfg.telegram, k, v)
            if "bridge" in raw:
                for k, v in raw["bridge"].items():
                    if hasattr(cfg.bridge, k):
                        setattr(cfg.bridge, k, v)
            if "log_to_brain" in raw:
                cfg.log_to_brain = raw["log_to_brain"]
        except Exception as e:
            log.warning(f"Failed to parse config file {CONFIG_PATH}: {e} — using defaults")

    # Env overrides (highest priority)
    if os.environ.get("TELEGRAM_BOT_TOKEN"):
        cfg.telegram.bot_token = os.environ["TELEGRAM_BOT_TOKEN"]
    if os.environ.get("TELEGRAM_ALLOWED_USERS"):
        cfg.telegram.allowed_user_ids = [
            int(x.strip())
            for x in os.environ["TELEGRAM_ALLOWED_USERS"].split(",")
            if x.strip()
        ]
    if os.environ.get("TELEGRAM_ALLOWED_CHATS"):
        cfg.telegram.allowed_chat_ids = [
            int(x.strip())
            for x in os.environ["TELEGRAM_ALLOWED_CHATS"].split(",")
            if x.strip()
        ]
    if os.environ.get("SWITCHAI_KEY"):
        cfg.bridge.switchai_api_key = os.environ["SWITCHAI_KEY"]
    if os.environ.get("ANTHROPIC_API_KEY"):
        cfg.bridge.anthropic_api_key = os.environ["ANTHROPIC_API_KEY"]
    if os.environ.get("SWITCHAI_MODEL"):
        cfg.bridge.switchai_model = os.environ["SWITCHAI_MODEL"]

    return cfg


def save_config(cfg: ChatConfig):
    """Persist config to disk."""
    CHAT_DATA_DIR.mkdir(parents=True, exist_ok=True)
    data = {
        "telegram": {
            "bot_token": cfg.telegram.bot_token,
            "allowed_user_ids": cfg.telegram.allowed_user_ids,
            "allowed_chat_ids": cfg.telegram.allowed_chat_ids,
            "polling_timeout": cfg.telegram.polling_timeout,
            "ignore_bots": cfg.telegram.ignore_bots,
        },
        "bridge": {
            "switchai_url": cfg.bridge.switchai_url,
            "switchai_model": cfg.bridge.switchai_model,
            "switchai_api_key": cfg.bridge.switchai_api_key,
            "anthropic_model": cfg.bridge.anthropic_model,
            "max_history_messages": cfg.bridge.max_history_messages,
            "max_tokens": cfg.bridge.max_tokens,
        },
        "log_to_brain": cfg.log_to_brain,
    }
    CONFIG_PATH.write_text(json.dumps(data, indent=2))
