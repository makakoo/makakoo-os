"""
Harvey OS — Telemetry Package

Event tracking and feature flag management.
"""

from .events import TelemetryEmitter, EventType, Event, emit_event, get_emitter
from .feature_flags import (
    FeatureFlagRegistry,
    FeatureFlag,
    get_flag_registry,
    is_feature_enabled,
)

__all__ = [
    "TelemetryEmitter",
    "EventType",
    "Event",
    "emit_event",
    "get_emitter",
    "FeatureFlagRegistry",
    "FeatureFlag",
    "get_flag_registry",
    "is_feature_enabled",
]
