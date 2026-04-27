"""Slack bot config — defaults to None until a workspace is added."""

from dataclasses import dataclass
from typing import Optional


@dataclass
class SlackConfig:
    """Slack bot credentials.

    Real implementation will populate these from env vars:
        SLACK_BOT_TOKEN          (xoxb-...)
        SLACK_SIGNING_SECRET     (for request verification)
        SLACK_APP_TOKEN          (xapp-... for Socket Mode)
    """

    bot_token: Optional[str] = None
    signing_secret: Optional[str] = None
    app_token: Optional[str] = None

    def is_configured(self) -> bool:
        return bool(self.bot_token and self.signing_secret)
