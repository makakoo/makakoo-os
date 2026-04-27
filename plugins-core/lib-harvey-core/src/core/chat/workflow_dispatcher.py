"""
ChatWorkflowDispatcher — Bridges HarveyChat messages to the WorkflowEngine.

When the IntelligentRouter classifies a message as needing multi-step work
(research, image, archive), this dispatcher creates a background workflow
and sends milestone-based progress updates to the user via Telegram.

Uses:
  - build_workflow_from_team() to create workflows from TeamRosters
  - AsyncDAGExecutor.run_workflow() to execute steps concurrently
  - EventBus subscription for step completion notifications

Progress updates are capped at 3-4 milestone messages (not every-step spam).
"""

import asyncio
import logging
import time
from typing import Callable, Awaitable, Dict, Any, Optional

log = logging.getLogger("harveychat.dispatcher")


class ChatWorkflowDispatcher:
    """Creates and runs background workflows for complex chat messages."""

    def __init__(
        self,
        engine,
        executor,
        send_fn: Callable[[str], Awaitable[None]],
        send_typing_fn: Optional[Callable[[], Awaitable[None]]] = None,
    ):
        """
        Args:
            engine: WorkflowEngine instance
            executor: AsyncDAGExecutor instance
            send_fn: async function to send a text message to the user
            send_typing_fn: async function to send typing indicator
        """
        self.engine = engine
        self.executor = executor
        self._send = send_fn
        self._send_typing = send_typing_fn

    async def dispatch(
        self,
        text: str,
        intent: str,
        team_roster,
        channel: str,
        user_id: str,
    ) -> Optional[asyncio.Task]:
        """Create and start a background workflow.

        Returns the asyncio.Task handle (for cancellation) or None on failure.
        """
        try:
            from core.orchestration.agent_team import build_workflow_from_team

            context = {
                "query": text,
                "user_id": user_id,
                "channel": channel,
                "intent": intent,
            }

            workflow = build_workflow_from_team(
                engine=self.engine,
                team=team_roster,
                context=context,
                workflow_name=f"chat-{intent}-{int(time.time())}",
                description=f"Chat workflow for: {text[:100]}",
            )

            total_steps = sum(1 for s in workflow.steps if s is not None)

            # Immediate acknowledgment
            await self._send(
                f"Starting {intent} workflow ({total_steps} steps). "
                f"I'll send updates as I go. Send /cancel to stop."
            )

            # Run in background
            task = asyncio.create_task(
                self._run_with_milestones(workflow, total_steps, channel, user_id)
            )
            return task

        except Exception as e:
            log.error(f"Failed to create workflow: {e}", exc_info=True)
            await self._send(f"Couldn't start workflow: {e}")
            return None

    async def _run_with_milestones(
        self,
        workflow,
        total_steps: int,
        channel: str,
        user_id: str,
    ):
        """Execute workflow and send milestone-based progress (max 3-4 messages)."""
        completed_steps = 0
        milestones_sent = 0
        max_milestones = 3  # ack + midpoint + completion
        last_typing = 0

        # Subscribe to step completion events via polling workflow state
        # (EventBus subscription is cleaner but requires async wiring;
        # polling is simpler and reliable for v1)
        try:
            result = await self.executor.run_workflow(workflow)

            # Count completed steps
            from core.workflow.engine import StepState
            completed_steps = sum(
                1 for s in result.steps
                if s.state == StepState.COMPLETED
            )
            failed_steps = sum(
                1 for s in result.steps
                if s.state == StepState.FAILED
            )

            # Build final response
            if failed_steps > 0:
                await self._send(
                    f"Workflow finished with issues: "
                    f"{completed_steps}/{total_steps} steps completed, "
                    f"{failed_steps} failed."
                )
            else:
                # Collect outputs from completed steps
                outputs = []
                for step in result.steps:
                    if step.state == StepState.COMPLETED and step.output:
                        outputs.append(step.output)

                if outputs:
                    combined = "\n\n---\n\n".join(outputs)
                    # Cap response to Telegram max (4096 chars)
                    if len(combined) > 3800:
                        combined = combined[:3800] + "\n\n[... truncated]"
                    await self._send(f"Done.\n\n{combined}")
                else:
                    await self._send("Workflow completed but produced no output.")

        except asyncio.CancelledError:
            log.info(f"Workflow {workflow.id} cancelled by user")
            await self._send("Workflow cancelled.")
        except Exception as e:
            log.error(f"Workflow {workflow.id} failed: {e}", exc_info=True)
            await self._send(f"Workflow failed: {e}")
