#!/usr/bin/env python3
"""
Superbrain Ingest — Unified entry point for all data ingestion.

Routes data to the correct system:
- Documents/media → Qdrant
- Brain pages/journals → PostgreSQL brain_embeddings
- Structured events → PostgreSQL events table
- All sources → entity extraction → Brain pages

Usage:
    python3 ingest.py /path/to/file.pdf
    python3 ingest.py --event '{"type": "trade", "agent": "arbitrage-agent", "summary": "BTC +$500"}'
"""

import json
import logging
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Optional

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

from core.superbrain import config
from core.superbrain import db
from core.superbrain.embeddings import embed_text

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("superbrain.ingest")


def ingest_event(
    event_type: str,
    agent: str,
    summary: str,
    details: dict = None,
    write_to_brain: bool = True,
) -> bool:
    """
    Ingest a structured event into the Superbrain.

    1. Embeds summary → PostgreSQL events table
    2. Writes to today's Brain journal (optional)

    Args:
        event_type: "trade", "email", "meeting", "deploy", "decision", etc.
        agent: Which agent produced this (e.g., "arbitrage-agent")
        summary: Human-readable summary
        details: Structured data (JSON-serializable)
        write_to_brain: Also write to Brain journal
    """
    details = details or {}

    # Embed summary
    embedding = embed_text(summary)
    if embedding is None:
        log.error("Failed to embed event: %s", summary[:80])
        return False

    # Insert into events table
    db.execute(
        """
        INSERT INTO events (event_type, agent, summary, details, embedding, occurred_at)
        VALUES (%s, %s, %s, %s, %s::vector, NOW())
        """,
        (event_type, agent, summary, json.dumps(details), str(embedding)),
    )
    log.info("Event ingested: [%s/%s] %s", event_type, agent, summary[:60])

    # Write to Brain journal
    if write_to_brain:
        _write_brain_journal(
            f"- [{event_type}] [[{agent}]]: {summary}"
        )

    # Log ingestion
    db.execute(
        """
        INSERT INTO ingestion_log (source_path, source_type, target_system, status, processed_at)
        VALUES (%s, %s, %s, %s, NOW())
        """,
        (f"event:{event_type}/{agent}", "event", "postgresql", "done"),
    )

    return True


def ingest_trade(
    symbol: str,
    direction: str,
    entry_price: float = None,
    exit_price: float = None,
    quantity: float = None,
    pnl: float = None,
    strategy: str = None,
    opened_at: datetime = None,
    closed_at: datetime = None,
    metadata: dict = None,
) -> bool:
    """Ingest a trade into the trades table + event log."""
    metadata = metadata or {}

    db.execute(
        """
        INSERT INTO trades (symbol, direction, entry_price, exit_price, quantity, pnl, strategy, opened_at, closed_at, metadata)
        VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
        """,
        (symbol, direction, entry_price, exit_price, quantity, pnl, strategy,
         opened_at, closed_at, json.dumps(metadata)),
    )

    # Also log as event
    pnl_str = f"PnL: ${pnl:+.2f}" if pnl is not None else ""
    summary = f"{symbol} {direction} trade {'closed' if closed_at else 'opened'} {pnl_str} (strategy: {strategy or 'unknown'})"
    ingest_event("trade", "arbitrage-agent", summary, {
        "symbol": symbol, "direction": direction, "pnl": pnl, "strategy": strategy
    })

    return True


def ingest_crm_lead(
    company: str,
    contact_name: str = None,
    contact_email: str = None,
    role_title: str = None,
    status: str = "new",
    source: str = None,
    notes: str = None,
) -> bool:
    """Ingest a CRM lead into the crm_leads table."""
    embed_text_content = f"{company} {role_title or ''} {contact_name or ''} {notes or ''}"
    embedding = embed_text(embed_text_content)

    db.execute(
        """
        INSERT INTO crm_leads (company, contact_name, contact_email, role_title, status, source, notes, embedding)
        VALUES (%s, %s, %s, %s, %s, %s, %s, %s::vector)
        """,
        (company, contact_name, contact_email, role_title, status, source, notes,
         str(embedding) if embedding else None),
    )

    # Also log as event
    ingest_event("crm_lead", "career-manager",
                 f"New lead: {contact_name or 'unknown'} at {company} ({role_title or 'unknown role'})",
                 {"company": company, "status": status, "source": source})

    return True


def _write_brain_journal(entry: str):
    """Append entry to today's Brain journal."""
    today = datetime.now().strftime("%Y_%m_%d")
    journal_path = Path(config.BRAIN_JOURNALS_DIR) / f"{today}.md"

    try:
        # Ensure entry starts with "- " (Brain outliner)
        if not entry.startswith("- "):
            entry = f"- {entry}"
        if not entry.endswith("\n"):
            entry += "\n"

        with open(journal_path, "a") as f:
            f.write(entry)
        log.debug("Written to Brain journal: %s", entry[:60])
    except Exception as e:
        log.error("Failed to write Brain journal: %s", e)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python3 ingest.py --event '{\"type\":\"trade\", \"agent\":\"arb\", \"summary\":\"BTC +$500\"}'")
        print("  python3 ingest.py --trade '{\"symbol\":\"BTC\", \"direction\":\"long\", \"pnl\":500}'")
        sys.exit(1)

    db.init_schema()

    if sys.argv[1] == "--event":
        data = json.loads(sys.argv[2])
        ingest_event(
            event_type=data["type"],
            agent=data["agent"],
            summary=data["summary"],
            details=data.get("details", {}),
        )
    elif sys.argv[1] == "--trade":
        data = json.loads(sys.argv[2])
        ingest_trade(**data)
    else:
        print(f"Unknown command: {sys.argv[1]}")
