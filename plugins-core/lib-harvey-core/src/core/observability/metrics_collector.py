"""
metrics_collector.py — Phase 4 deliverable

MetricsCollector: scrapes every Phase 3 observability surface
(ResourceMonitor, FailureRecovery, AgentCoordinator, PersistentEventBus,
ArtifactStore) and renders Prometheus text format. Designed to be
called by a /metrics HTTP handler or written to disk on a timer.

Not a full Prometheus client library — there's no histogram, no label
registry, no HTTP server here. Deliberately tiny (~300 lines) because
the Phase 3 modules already expose all the raw state we need.

Exposed:
  MetricPoint    — single metric sample
  MetricsCollector — scrape + render
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

log = logging.getLogger("harvey.metrics")


@dataclass
class MetricPoint:
    """A single Prometheus-compatible metric sample."""
    name: str                           # e.g. "harvey_cpu_percent"
    value: float
    help_text: str = ""
    metric_type: str = "gauge"          # gauge | counter
    labels: Dict[str, str] = field(default_factory=dict)

    def render(self) -> str:
        """Render this point as a Prometheus exposition line."""
        if self.labels:
            label_str = ",".join(
                f'{k}="{_escape(str(v))}"' for k, v in sorted(self.labels.items())
            )
            return f"{self.name}{{{label_str}}} {self.value}"
        return f"{self.name} {self.value}"


def _escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


class MetricsCollector:
    """
    Scrapes Harvey swarm state into Prometheus text format.

    Bolt every source onto one collector and call `render()` (or
    `snapshot()` for programmatic access). Threads are safe: render()
    takes a snapshot under a lock.

    Typical wiring:

        collector = MetricsCollector(
            resource_monitor=monitor,
            failure_recovery=recovery,
            coordinator=coord,
            artifact_store=store,
            event_bus=bus,
        )
        metrics_text = collector.render()
    """

    def __init__(
        self,
        resource_monitor=None,
        failure_recovery=None,
        coordinator=None,
        artifact_store=None,
        event_bus=None,
        namespace: str = "harvey",
    ):
        self.resource_monitor = resource_monitor
        self.failure_recovery = failure_recovery
        self.coordinator = coordinator
        self.artifact_store = artifact_store
        self.event_bus = event_bus
        self.namespace = namespace
        self._lock = threading.RLock()
        self._scrape_count = 0
        self._last_scrape_at: float = 0.0
        self._scrape_errors = 0

    # ─── Scrape ──────────────────────────────────────────────────

    def snapshot(self) -> List[MetricPoint]:
        """Collect all metrics from every bound source. Thread-safe."""
        with self._lock:
            points: List[MetricPoint] = []

            points.append(MetricPoint(
                name=f"{self.namespace}_scrape_count_total",
                value=self._scrape_count,
                help_text="Total number of metric scrapes performed",
                metric_type="counter",
            ))
            points.append(MetricPoint(
                name=f"{self.namespace}_scrape_errors_total",
                value=self._scrape_errors,
                help_text="Total scrape errors",
                metric_type="counter",
            ))

            try:
                points.extend(self._scrape_resource_monitor())
                points.extend(self._scrape_failure_recovery())
                points.extend(self._scrape_coordinator())
                points.extend(self._scrape_artifact_store())
                points.extend(self._scrape_event_bus())
            except Exception as e:
                log.warning(f"[metrics] scrape partial failure: {e}")
                self._scrape_errors += 1

            self._scrape_count += 1
            self._last_scrape_at = time.time()
            return points

    def render(self) -> str:
        """
        Render a Prometheus text-format exposition string. Groups points
        by name and emits `# HELP` + `# TYPE` headers above each group.
        """
        points = self.snapshot()
        if not points:
            return ""

        # Group by name to emit one HELP/TYPE block per unique name
        by_name: Dict[str, List[MetricPoint]] = {}
        for p in points:
            by_name.setdefault(p.name, []).append(p)

        lines: List[str] = []
        for name in sorted(by_name.keys()):
            group = by_name[name]
            first = group[0]
            if first.help_text:
                lines.append(f"# HELP {name} {first.help_text}")
            lines.append(f"# TYPE {name} {first.metric_type}")
            for p in group:
                lines.append(p.render())

        return "\n".join(lines) + "\n"

    # ─── Sources ─────────────────────────────────────────────────

    def _scrape_resource_monitor(self) -> List[MetricPoint]:
        if self.resource_monitor is None:
            return []
        snap = self.resource_monitor.latest() or self.resource_monitor.sample()
        ns = self.namespace
        return [
            MetricPoint(
                name=f"{ns}_cpu_percent", value=snap.cpu_percent,
                help_text="System CPU utilization (0-100)",
            ),
            MetricPoint(
                name=f"{ns}_mem_percent", value=snap.mem_percent,
                help_text="System memory utilization (0-100)",
            ),
            MetricPoint(
                name=f"{ns}_running_agents", value=snap.running_agents,
                help_text="Number of registered subagents",
            ),
            MetricPoint(
                name=f"{ns}_in_flight_steps", value=snap.in_flight_steps,
                help_text="DAG steps currently executing",
            ),
        ]

    def _scrape_failure_recovery(self) -> List[MetricPoint]:
        if self.failure_recovery is None:
            return []
        status = self.failure_recovery.status()
        ns = self.namespace
        out: List[MetricPoint] = [
            MetricPoint(
                name=f"{ns}_breaker_count", value=status["breaker_count"],
                help_text="Total number of agent circuit breakers",
            ),
            MetricPoint(
                name=f"{ns}_breakers_tripped",
                value=len(self.failure_recovery.tripped_agents()),
                help_text="Number of circuit breakers currently OPEN",
            ),
        ]
        # Per-breaker state (label = agent name)
        for agent_name, b in status.get("breakers", {}).items():
            state_value = {"closed": 0, "half_open": 1, "open": 2}.get(
                b["state"], -1
            )
            out.append(MetricPoint(
                name=f"{ns}_breaker_state",
                value=state_value,
                help_text="Breaker state: 0=closed, 1=half_open, 2=open",
                labels={"agent": agent_name},
            ))
            out.append(MetricPoint(
                name=f"{ns}_breaker_total_failures",
                value=b["total_failures"],
                help_text="Cumulative failures per agent breaker",
                metric_type="counter",
                labels={"agent": agent_name},
            ))
            out.append(MetricPoint(
                name=f"{ns}_breaker_total_successes",
                value=b["total_successes"],
                help_text="Cumulative successes per agent breaker",
                metric_type="counter",
                labels={"agent": agent_name},
            ))
        return out

    def _scrape_coordinator(self) -> List[MetricPoint]:
        if self.coordinator is None:
            return []
        status = self.coordinator.status()
        ns = self.namespace
        out = [
            MetricPoint(
                name=f"{ns}_agent_count", value=status.get("agent_count", 0),
                help_text="Registered agent count on the coordinator",
            ),
            MetricPoint(
                name=f"{ns}_artifact_count",
                value=status.get("artifact_count", 0),
                help_text="Artifacts present in the store",
            ),
            MetricPoint(
                name=f"{ns}_event_count",
                value=status.get("event_count", 0),
                help_text="Events present in the bus",
                metric_type="counter",
            ),
            MetricPoint(
                name=f"{ns}_latest_event_seq",
                value=status.get("latest_seq", 0),
                help_text="Highest event bus sequence number",
                metric_type="counter",
            ),
            MetricPoint(
                name=f"{ns}_olibia_commentary_total",
                value=status.get("olibia_commentary", 0),
                help_text="Total Olibia commentary events",
                metric_type="counter",
            ),
        ]
        # TaskMaster progress snapshot (per-step state)
        progress = status.get("progress", {})
        state_tallies: Dict[str, int] = {}
        for state in progress.values():
            state_tallies[state] = state_tallies.get(state, 0) + 1
        for state, count in state_tallies.items():
            out.append(MetricPoint(
                name=f"{ns}_step_state_count",
                value=count,
                help_text="Step count bucketed by state",
                labels={"state": state},
            ))
        return out

    def _scrape_artifact_store(self) -> List[MetricPoint]:
        if self.artifact_store is None:
            return []
        try:
            count = self.artifact_store.count()
        except Exception:
            return []
        return [
            MetricPoint(
                name=f"{self.namespace}_artifact_store_size",
                value=count,
                help_text="Number of artifacts in the store",
            ),
        ]

    def _scrape_event_bus(self) -> List[MetricPoint]:
        if self.event_bus is None:
            return []
        try:
            count = self.event_bus.count()
            latest = self.event_bus.latest_seq()
        except Exception:
            return []
        return [
            MetricPoint(
                name=f"{self.namespace}_event_bus_size",
                value=count,
                help_text="Number of events in the bus",
            ),
            MetricPoint(
                name=f"{self.namespace}_event_bus_latest_seq",
                value=latest,
                help_text="Latest monotonic event sequence number",
                metric_type="counter",
            ),
        ]

    # ─── Status ──────────────────────────────────────────────────

    def status(self) -> Dict[str, Any]:
        with self._lock:
            return {
                "scrape_count": self._scrape_count,
                "scrape_errors": self._scrape_errors,
                "last_scrape_at": self._last_scrape_at,
                "namespace": self.namespace,
                "sources": {
                    "resource_monitor": self.resource_monitor is not None,
                    "failure_recovery": self.failure_recovery is not None,
                    "coordinator": self.coordinator is not None,
                    "artifact_store": self.artifact_store is not None,
                    "event_bus": self.event_bus is not None,
                },
            }


__all__ = ["MetricPoint", "MetricsCollector"]
