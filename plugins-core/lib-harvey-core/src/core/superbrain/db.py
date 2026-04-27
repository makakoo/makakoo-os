"""⚠️ DEPRECATED — PostgreSQL connection helper for superbrain.

Superbrain migrated from PostgreSQL to SQLite FTS5 (store.py) in 2026-04.
The Rust equivalent at makakoo-os/makakoo-core/src/superbrain/ reads the
same superbrain.db file. This PostgreSQL helper is only used by legacy
migration scripts (backfill_anchors, etc.). No new code should import this.
"""

import psycopg2
import psycopg2.extras
import logging
from typing import List, Dict, Any, Optional
from contextlib import contextmanager

from . import config

log = logging.getLogger("superbrain.db")


def get_connection():
    """Get a PostgreSQL connection to harvey_brain."""
    return psycopg2.connect(
        host=config.PG_HOST,
        port=config.PG_PORT,
        dbname=config.PG_DB,
        user=config.PG_USER,
        password=config.PG_PASSWORD,
    )


@contextmanager
def get_cursor(commit=True):
    """Context manager for database cursor with auto-commit."""
    conn = get_connection()
    try:
        cur = conn.cursor(cursor_factory=psycopg2.extras.RealDictCursor)
        yield cur
        if commit:
            conn.commit()
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()


def execute(sql: str, params: tuple = None) -> None:
    """Execute a SQL statement (INSERT, UPDATE, CREATE, etc.)."""
    with get_cursor() as cur:
        cur.execute(sql, params)


def query(sql: str, params: tuple = None) -> List[Dict[str, Any]]:
    """Execute a SQL query and return results as list of dicts."""
    with get_cursor(commit=False) as cur:
        cur.execute(sql, params)
        return cur.fetchall()


def query_one(sql: str, params: tuple = None) -> Optional[Dict[str, Any]]:
    """Execute a SQL query and return first result or None."""
    with get_cursor(commit=False) as cur:
        cur.execute(sql, params)
        return cur.fetchone()


def init_schema():
    """Run schema.sql to create all superbrain tables and indexes."""
    import os
    schema_path = os.path.join(os.path.dirname(__file__), "schema.sql")

    with open(schema_path) as f:
        sql = f.read()

    with get_cursor() as cur:
        cur.execute(sql)
    log.info("Superbrain schema initialized")
