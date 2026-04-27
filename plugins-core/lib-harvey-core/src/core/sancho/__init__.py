"""
SANCHO — Proactive engine for Harvey OS.

Runs maintenance and enrichment tasks autonomously when conditions are met.
Named for the Greek concept of "the opportune moment."

Exports:
    Sancho        — Main engine (tick, run_once)
    ProactiveTask — Task definition dataclass
    GateSystem    — Composable precondition system
    Gate          — Single precondition check
"""

from core.sancho.engine import Sancho
from core.sancho.gates import Gate, GateSystem
from core.sancho.tasks import ProactiveTask

__all__ = ["Sancho", "ProactiveTask", "GateSystem", "Gate"]
