"""
Harvey Workflow System — Multi-step task orchestration.

Three-tier task system:
  1. TaskQueue (core.chat.task_queue) — Q&A, single-step (~seconds to minutes)
  2. Workflow (core.workflow) — Multi-step DAG with checkpointing (~minutes to days)
  3. Campaign (future) — Multi-workflow coordination (~days to weeks)

This module provides:
- WorkflowEngine: DAG execution with persistent state
- WorkflowExecutor: Background executor with polling
- WorkflowTemplates: Pre-built common workflows
"""

from core.workflow.engine import (
    Workflow,
    WorkflowEngine,
    WorkflowState,
    WorkflowStep,
    StepState,
)
from core.workflow.executor import WorkflowExecutor, WorkflowTemplates

__all__ = [
    "Workflow",
    "WorkflowEngine",
    "WorkflowState",
    "WorkflowStep",
    "StepState",
    "WorkflowExecutor",
    "WorkflowTemplates",
]
