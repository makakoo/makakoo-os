"""
core/observability — Phase 4

Production monitoring + structured logging for the Harvey swarm.

Modules:
  metrics_collector — Prometheus text-format exporter over ResourceMonitor,
                      FailureRecovery, AgentCoordinator, ArtifactStore,
                      PersistentEventBus.
  structured_logger — JSON log formatter with workflow/step/agent context
                      vars bound around Subagent.handle().

Prefer importing from this package root:

    from core.observability import (
        MetricsCollector, configure_json_logging, log_context,
    )
"""

from core.observability.metrics_collector import (
    MetricPoint,
    MetricsCollector,
)
from core.observability.structured_logger import (
    JsonFormatter,
    bind_context,
    configure_json_logging,
    current_context,
    log_context,
    unbind_context,
)

__all__ = [
    # metrics
    "MetricPoint",
    "MetricsCollector",
    # structured logging
    "JsonFormatter",
    "bind_context",
    "configure_json_logging",
    "current_context",
    "log_context",
    "unbind_context",
]
