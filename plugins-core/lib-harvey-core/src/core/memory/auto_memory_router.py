#!/usr/bin/env python3
"""
Auto-Memory Router — Autonomous memory capture system.

Subscribes to EventBus and automatically:
1. Extracts facts from events
2. Embeds facts semantically
3. Extracts entities and metadata
4. Routes to Brain journal + Superbrain
5. Signals SANCHO consolidation when threshold met

Works with any CLI (Claude Code, Gemini, Codex, OpenCode) as universal Harvey infrastructure.

Usage:
    from core.memory.auto_memory_router import AutoMemoryRouter
    router = AutoMemoryRouter()
    router.start()  # Subscribe to EventBus
"""

import os
import sys
import logging
import json
import re
from datetime import datetime
from typing import Dict, Any, Optional, List

# Add parent to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from core.events import EventBus, Event
from core.memory import brain_bridge
from core.superbrain import embeddings as sb_embeddings
from core.superbrain import store as sb_store

log = logging.getLogger("harvey.auto_memory")


class AutoMemoryRouter:
    """
    Routes all significant events to memory systems:
    - Brain journal (raw facts)
    - Superbrain (semantic index)
    - SANCHO consolidation (when thresholds met)
    """

    def __init__(self, max_entries_per_day: int = 500):
        self.bus = EventBus.instance()
        self.max_entries_per_day = max_entries_per_day
        self.entries_today = 0
        self.handlers = {
            "git.commit.*": self._handle_git_commit,
            "task.completed": self._handle_task_completed,
            "agent.spawned": self._handle_agent_spawned,
            "agent.died": self._handle_agent_died,
            "skill.*": self._handle_skill_invoked,
            "brain.*": self._handle_brain_write,
            "sancho.*": self._handle_sancho_task,
            "coordinator.*": self._handle_coordinator_phase,
            "session.start": self._handle_session_start,
            "session.end": self._handle_session_end,
            "email.received": self._handle_email_received,
            "calendar.event.*": self._handle_calendar_event,
            "chat.message": self._handle_chat_message,
            "trade.executed": self._handle_trade_executed,
            "file.operation": self._handle_file_operation,
        }

    def start(self):
        """Subscribe to all event patterns."""
        log.info("Starting Auto-Memory Router")
        for pattern in self.handlers.keys():
            self.bus.subscribe(pattern, self._route_event)

    def _route_event(self, event: Event):
        """
        Main event router. Called for all matching events.
        Extracts facts, enriches, and routes to memory systems.
        Runs synchronously but offloads heavy work to avoid blocking.
        """
        try:
            # Find matching handler
            import fnmatch
            handler = None
            for pattern, h in self.handlers.items():
                if fnmatch.fnmatch(event.type, pattern):
                    handler = h
                    break

            if not handler:
                log.debug(f"No handler for event: {event.type}")
                return

            # Extract facts using handler
            facts = handler(event)
            if not facts:
                log.debug(f"Handler filtered event: {event.type}")
                return

            # Enrich with metadata
            facts["_timestamp"] = datetime.now().isoformat()
            facts["_source_event"] = event.type
            facts["_source_agent"] = event.source

            # Try to embed (non-blocking on failure)
            try:
                embedding = self._embed_facts(facts)
                facts["_embedding"] = embedding
            except Exception as e:
                log.warning(f"Failed to embed facts: {e}")
                facts["_embedding"] = None

            # Extract entities (wikilinks)
            entities = self._extract_entities(facts)
            facts["_entities"] = entities

            # Calculate significance (0-1 score)
            significance = self._calculate_significance(event.type, facts)
            facts["_significance"] = significance

            # Route to systems
            self._log_to_journal(facts)
            self._index_in_superbrain(facts)

            # Emit memory.captured event for indexer to subscribe to
            self.bus.publish(
                "memory.captured",
                source="auto_memory_router",
                fact=facts
            )

            # Increment counter and check threshold
            self.entries_today += 1
            if self.entries_today % 5 == 0:
                self._signal_sancho_consolidation()

            log.info(f"Auto-memory captured: {event.type} (significance: {significance:.2f})")

        except Exception as e:
            log.error(f"Error in auto-memory router: {e}", exc_info=True)

    # ── Event Handlers ────────────────────────────────────────

    def _handle_git_commit(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from git commit."""
        data = event.data
        return {
            "type": "decision",
            "category": "development",
            "action": "commit",
            "message": data.get("message", ""),
            "files_changed": data.get("files", []),
            "insertions": data.get("insertions", 0),
            "deletions": data.get("deletions", 0),
            "author": data.get("author", ""),
        }

    def _handle_task_completed(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from task completion."""
        data = event.data
        return {
            "type": "milestone",
            "category": "project",
            "action": "task_complete",
            "task_id": data.get("id", ""),
            "subject": data.get("subject", ""),
            "duration_minutes": data.get("duration", 0),
            "status": data.get("status", "completed"),
            "blocked_by": data.get("blocked_by", []),
        }

    def _handle_agent_spawned(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from agent spawn."""
        data = event.data
        return {
            "type": "system_event",
            "category": "infrastructure",
            "action": "agent_spawn",
            "agent_name": data.get("name", ""),
            "role": data.get("role", ""),
            "pid": data.get("pid", 0),
        }

    def _handle_agent_died(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from agent death."""
        data = event.data
        return {
            "type": "system_event",
            "category": "infrastructure",
            "action": "agent_died",
            "agent_name": data.get("name", ""),
            "reason": data.get("reason", ""),
            "uptime_seconds": data.get("uptime", 0),
        }

    def _handle_skill_invoked(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from skill execution."""
        data = event.data
        return {
            "type": "capability_usage",
            "category": "skills",
            "action": "skill_execute",
            "skill_name": data.get("name", ""),
            "skill_category": data.get("category", ""),
            "parameters": data.get("params", {}),
            "result_status": data.get("status", "unknown"),
        }

    def _handle_brain_write(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from Brain writes."""
        data = event.data
        return {
            "type": "knowledge",
            "category": "brain",
            "action": "brain_write",
            "page_name": data.get("page", ""),
            "content_summary": (data.get("content", "")[:200] + "...")
            if data.get("content") else "",
            "content_length": len(data.get("content", "")),
        }

    def _handle_sancho_task(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from SANCHO proactive tasks."""
        data = event.data
        return {
            "type": "system_event",
            "category": "proactive",
            "action": "sancho_task",
            "task_id": data.get("task_id", ""),
            "task_name": data.get("name", ""),
            "result": data.get("result", ""),
        }

    def _handle_coordinator_phase(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from coordinator swarm phases."""
        data = event.data
        return {
            "type": "coordination",
            "category": "swarm",
            "action": "coordinator_phase",
            "phase": data.get("phase", ""),
            "task_count": data.get("task_count", 0),
            "result": data.get("result", ""),
        }

    def _handle_session_start(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from session start."""
        data = event.data
        return {
            "type": "session_event",
            "category": "session",
            "action": "session_start",
            "session_id": data.get("session_id", ""),
            "agent_id": event.source,
        }

    def _handle_session_end(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from session end."""
        data = event.data
        return {
            "type": "session_event",
            "category": "session",
            "action": "session_end",
            "session_id": data.get("session_id", ""),
            "duration_minutes": data.get("duration", 0),
        }

    def _handle_email_received(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from email arrival."""
        data = event.data
        return {
            "type": "external_event",
            "category": "communication",
            "action": "email_received",
            "sender": data.get("sender", "Unknown"),
            "subject": data.get("subject", ""),
            "snippet": data.get("snippet", "")[:150],
        }

    def _handle_calendar_event(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from calendar event."""
        data = event.data
        return {
            "type": "external_event",
            "category": "scheduling",
            "action": "calendar_event",
            "title": data.get("title", ""),
            "attendees": ", ".join(data.get("attendees", [])[:3]),
            "time": data.get("start_time", ""),
        }

    def _handle_chat_message(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from chat message."""
        data = event.data
        if not data.get("is_significant"):
            return None
        return {
            "type": "external_event",
            "category": "communication",
            "action": "chat_message",
            "platform": data.get("platform", ""),
            "channel": data.get("channel", ""),
            "author": data.get("author", ""),
            "content": data.get("content", "")[:150],
        }

    def _handle_trade_executed(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from trade execution."""
        data = event.data
        return {
            "type": "decision",
            "category": "trading",
            "action": "trade_executed",
            "symbol": data.get("symbol", ""),
            "direction": data.get("direction", ""),
            "pnl": data.get("pnl", ""),
            "reasoning": data.get("reasoning", "")[:100],
        }

    def _handle_file_operation(self, event: Event) -> Optional[Dict[str, Any]]:
        """Extract facts from file operations."""
        data = event.data
        return {
            "type": "capability",
            "category": "system",
            "action": "file_operation",
            "operation": data.get("operation", ""),
            "file_type": data.get("file_type", ""),
            "name": data.get("name", ""),
            "description": data.get("description", "")[:100],
        }

    # ── Enrichment ────────────────────────────────────────────

    def _embed_facts(self, facts: Dict[str, Any]) -> Optional[List[float]]:
        """
        Convert facts to text and embed using Superbrain embeddings.
        Falls back gracefully if embedding fails.
        """
        try:
            # Create text representation of facts
            text_parts = []
            for k, v in facts.items():
                if not k.startswith("_"):
                    if isinstance(v, (str, int, float)):
                        text_parts.append(f"{k}: {v}")
                    elif isinstance(v, list):
                        text_parts.append(f"{k}: {', '.join(str(x) for x in v)}")

            text = " ".join(text_parts)
            if len(text) < 10:
                return None

            # Use Superbrain embeddings (switchAILocal mistral-embed)
            embedding = sb_embeddings.embed_text(text)
            return embedding

        except Exception as e:
            log.warning(f"Embedding failed: {e}")
            return None

    def _extract_entities(self, facts: Dict[str, Any]) -> List[str]:
        """
        Extract entity references from facts.
        Looks for wikilink-style references [[Entity]] in facts.
        """
        entities = []
        for v in facts.values():
            if isinstance(v, str):
                # Find [[entity]] patterns
                matches = re.findall(r"\[\[([^\]]+)\]\]", v)
                entities.extend(matches)
            elif isinstance(v, list):
                for item in v:
                    if isinstance(item, str):
                        matches = re.findall(r"\[\[([^\]]+)\]\]", item)
                        entities.extend(matches)

        return list(set(entities))  # Remove duplicates

    def _calculate_significance(self, event_type: str, facts: Dict[str, Any]) -> float:
        """
        Calculate significance score (0-1) for facts.
        Higher score = more important to capture and consolidate.
        """
        score = 0.5  # baseline

        # Event type boosts
        if event_type.startswith("git.commit"):
            score += 0.3  # Git commits always important
        elif event_type == "task.completed":
            score += 0.2  # Task completions important
        elif event_type == "agent.spawned" or event_type == "agent.died":
            score += 0.1  # Agent lifecycle moderately important
        elif event_type.startswith("skill"):
            score += 0.15  # Skill usage moderately important

        # Content-based boosts
        fact_type = facts.get("type", "")
        if fact_type == "decision":
            score += 0.2
        elif fact_type == "milestone":
            score += 0.15
        elif fact_type == "knowledge":
            score += 0.1

        # Clamp to [0, 1]
        return min(max(score, 0.0), 1.0)

    # ── Persistence ───────────────────────────────────────────

    def _log_to_journal(self, facts: Dict[str, Any]):
        """
        Log facts to Brain journal (raw capture).
        Appends to today's journal with formatting.
        """
        try:
            # Format as Brain entry
            entry = self._format_journal_entry(facts)
            if not entry:
                return

            # Append to today's journal
            result = brain_bridge.log_to_today_journal(
                entry,
                tags=["auto-memory", facts.get("category", "unknown")]
            )

            if not result:
                log.warning("Failed to write to journal")

        except Exception as e:
            log.error(f"Error writing to journal: {e}")

    def _index_in_superbrain(self, facts: Dict[str, Any]):
        """
        Index facts in Superbrain (semantic search).
        Phase 1: Just log that indexing was attempted. Phase 2 will implement actual indexing.

        Superbrain store uses Logseq Brain files as source of truth anyway,
        so the journal entry IS the indexed fact. This method is a placeholder
        for future semantic indexing via vector embeddings.
        """
        try:
            embedding = facts.get("_embedding")
            if not embedding:
                log.debug(f"Skipping superbrain index (no embedding)")
                return

            # TODO: Phase 2 - implement vector indexing
            # For now, the fact has already been logged to journal,
            # which is the source of truth for Superbrain.
            log.debug(f"Fact embedded ({len(embedding)} dims) - Phase 2 will index to superbrain")

        except Exception as e:
            log.debug(f"Error in superbrain indexing prep: {e}")

    def _signal_sancho_consolidation(self):
        """
        Signal SANCHO to run memory consolidation task.
        Fired when entry count reaches threshold (every 5 entries).
        """
        try:
            # Emit event that SANCHO listens for
            self.bus.publish(
                "memory.consolidation.requested",
                source="auto_memory_router",
                entries_captured=self.entries_today
            )
            log.info(f"Signaled SANCHO consolidation ({self.entries_today} entries)")
        except Exception as e:
            log.warning(f"Failed to signal SANCHO: {e}")

    # ── Formatting ────────────────────────────────────────────

    def _format_journal_entry(self, facts: Dict[str, Any]) -> Optional[str]:
        """
        Format facts as Brain outliner entry with metadata and wikilinks.
        """
        try:
            source_event = facts.get("_source_event", "unknown")
            timestamp = facts.get("_timestamp", "")
            action = facts.get("action", "")
            entities = facts.get("_entities", [])

            # Build entry
            parts = [f"[{source_event.split('.')[-1]}]"]

            # Add main content based on type
            if action == "commit" and facts.get("message"):
                parts.append(f"Git commit: {facts['message'][:80]}")
            elif action == "task_complete" and facts.get("subject"):
                parts.append(f"Task completed: {facts['subject']}")
            elif action == "agent_spawn" and facts.get("agent_name"):
                parts.append(f"Agent spawned: {facts['agent_name']}")
            elif action == "skill_execute" and facts.get("skill_name"):
                parts.append(f"Skill executed: {facts['skill_name']}")
            elif action == "brain_write" and facts.get("page_name"):
                parts.append(f"Brain write: {facts['page_name']}")
            else:
                parts.append(facts.get("type", "event"))

            entry = " ".join(parts)

            # Add wikilinks for entities
            if entities:
                entity_links = " ".join(f"[[{e}]]" for e in entities[:5])
                entry += f" {entity_links}"

            return entry

        except Exception as e:
            log.warning(f"Error formatting journal entry: {e}")
            return None


# ═════════════════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    # Test the router
    logging.basicConfig(level=logging.DEBUG)

    router = AutoMemoryRouter()
    router.start()

    # Emit test event
    bus = EventBus.instance()
    bus.publish(
        "git.commit.created",
        source="test_agent",
        message="Test commit: fix auth middleware",
        files=["core/auth/middleware.py"],
        author="test@example.com"
    )

    print("Test event emitted. Check logs for auto-memory capture.")
