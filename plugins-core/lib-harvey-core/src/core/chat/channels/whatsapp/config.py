"""WhatsApp Business Cloud API config — all fields default to None until wired."""

from dataclasses import dataclass
from typing import Optional


@dataclass
class WhatsAppConfig:
    """Meta WhatsApp Business Cloud API credentials.

    Real implementation will populate these from env vars:
        WHATSAPP_PHONE_NUMBER_ID
        WHATSAPP_ACCESS_TOKEN
        WHATSAPP_WEBHOOK_VERIFY_TOKEN
        WHATSAPP_APP_SECRET
    """

    phone_number_id: Optional[str] = None
    access_token: Optional[str] = None
    webhook_verify_token: Optional[str] = None
    app_secret: Optional[str] = None

    def is_configured(self) -> bool:
        return bool(self.phone_number_id and self.access_token)
