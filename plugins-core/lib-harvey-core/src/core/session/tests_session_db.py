#!/usr/bin/env python3
"""
Tests for SessionDB - SQLite session storage with FTS5 full-text search.

Run with: python3 -m pytest plugins-core/lib-harvey-core/src/core/session/tests_session_db.py -v
"""

import os
import sys
import tempfile
import shutil
import pytest

# Add core to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", ".."))


class TestSessionDBCreation:
    """Test 1: SessionDB creates SQLite DB with FTS5 virtual table on init."""

    def test_session_db_creates_fts5_table(self):
        """SessionDB creates SQLite DB with FTS5 virtual table on init."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db_path = os.path.join(tmpdir, "test.db")
            db = SessionDB(db_path)

            # Verify DB file was created
            assert os.path.exists(db_path), "Database file should be created"

            # Verify FTS5 table exists by querying it
            conn = db._get_conn()
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='messages_fts'"
            )
            row = cursor.fetchone()
            assert row is not None, "FTS5 virtual table should exist"

            # Verify sessions table exists
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='sessions'"
            )
            row = cursor.fetchone()
            assert row is not None, "sessions table should exist"

            # Verify messages table exists
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='messages'"
            )
            row = cursor.fetchone()
            assert row is not None, "messages table should exist"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestInsertMessage:
    """Test 2: insert_message() stores message with session_id, role, content."""

    def test_insert_message_stores_fields(self):
        """insert_message() stores message with session_id, role, content."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            msg_id = db.insert_message(sid, "user", "hello world")
            assert msg_id is not None, "Message ID should be returned"

            # Verify message was stored
            conn = db._get_conn()
            cursor = conn.execute(
                "SELECT session_id, role, content FROM messages WHERE id = ?", (msg_id,)
            )
            row = cursor.fetchone()
            assert row is not None, "Message should be stored"
            assert row["session_id"] == sid
            assert row["role"] == "user"
            assert row["content"] == "hello world"

            db.close()
        finally:
            shutil.rmtree(tmpdir)

    def test_insert_message_with_tool_name(self):
        """insert_message() stores tool_name when provided."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            msg_id = db.insert_message(
                sid, "tool", "file result", tool_name="read_file"
            )

            conn = db._get_conn()
            cursor = conn.execute(
                "SELECT tool_name FROM messages WHERE id = ?", (msg_id,)
            )
            row = cursor.fetchone()
            assert row is not None
            assert row["tool_name"] == "read_file"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestSearchMessages:
    """Test 3 & 4: search_messages() returns FTS5 ranked results with role filter."""

    def test_search_messages_returns_fts5_results(self):
        """search_messages() returns FTS5 ranked results."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "hello world")
            db.insert_message(sid, "assistant", "hi there")
            db.insert_message(sid, "tool", "file result", tool_name="read_file")

            results = db.search_messages("hello")
            assert len(results) > 0, "Should find matching messages"
            assert any(
                "hello" in str(r.get("content", ""))
                or "hello" in str(r.get("snippet", ""))
                for r in results
            ), "Should find 'hello' in results"

            db.close()
        finally:
            shutil.rmtree(tmpdir)

    def test_search_messages_filters_by_role(self):
        """search_messages() filters by role_filter."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "hello world")
            db.insert_message(sid, "assistant", "hi there")
            db.insert_message(sid, "tool", "file result", tool_name="read_file")

            # Search only for user role
            results = db.search_messages("hello", role_filter=["user"])
            assert all(r["role"] == "user" for r in results), (
                "Should only return user messages"
            )

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestGetMessagesAsConversation:
    """Test 5: get_messages_as_conversation() returns full session as message list."""

    def test_get_messages_as_conversation(self):
        """get_messages_as_conversation() returns full session as message list."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "hello world")
            db.insert_message(sid, "assistant", "hi there")
            db.insert_message(sid, "tool", "file result", tool_name="read_file")

            messages = db.get_messages_as_conversation(sid)
            assert len(messages) == 3, "Should return all 3 messages"
            assert messages[0]["role"] == "user"
            assert messages[1]["role"] == "assistant"
            assert messages[2]["role"] == "tool"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestListSessions:
    """Test 6: list_sessions() returns session metadata."""

    def test_list_sessions(self):
        """list_sessions() returns session metadata."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))

            sid1 = db.create_session("session one")
            sid2 = db.create_session("session two")

            sessions = db.list_sessions(limit=10)
            assert len(sessions) == 2, "Should return 2 sessions"
            assert all("id" in s for s in sessions), "Each session should have id"
            assert all("title" in s for s in sessions), "Each session should have title"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestGetSession:
    """Test 7: get_session() returns single session metadata."""

    def test_get_session(self):
        """get_session() returns single session metadata."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            session = db.get_session(sid)
            assert session is not None, "Should return session"
            assert session["id"] == sid
            assert session["title"] == "test session"

            db.close()
        finally:
            shutil.rmtree(tmpdir)

    def test_get_session_nonexistent(self):
        """get_session() returns None for nonexistent session."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))

            session = db.get_session("nonexistent")
            assert session is None, "Should return None for nonexistent session"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestFTS5QueryHandling:
    """Test 8: FTS5 search handles OR/AND/phrase queries."""

    def test_fts5_or_query(self):
        """FTS5 search handles OR queries."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "python programming")
            db.insert_message(sid, "user", "javascript coding")

            # OR query should find both
            results = db.search_messages("python OR javascript")
            assert len(results) >= 1, "Should find messages with python or javascript"

            db.close()
        finally:
            shutil.rmtree(tmpdir)

    def test_fts5_phrase_query(self):
        """FTS5 search handles phrase queries."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "I love python programming")

            # Phrase query
            results = db.search_messages('"python programming"')
            assert len(results) >= 1, "Should find exact phrase"

            db.close()
        finally:
            shutil.rmtree(tmpdir)

    def test_fts5_prefix_query(self):
        """FTS5 search handles prefix queries."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))
            sid = db.create_session("test session")

            db.insert_message(sid, "user", "python programming")

            # Prefix query
            results = db.search_messages("pyth*")
            assert len(results) >= 1, "Should find prefix match"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


class TestWALMode:
    """Test WAL mode is enabled for concurrent access."""

    def test_wal_mode_enabled(self):
        """SQLite WAL mode is enabled."""
        from harvey_os.core.session.session_db import SessionDB

        tmpdir = tempfile.mkdtemp()
        try:
            db = SessionDB(os.path.join(tmpdir, "test.db"))

            conn = db._get_conn()
            cursor = conn.execute("PRAGMA journal_mode")
            row = cursor.fetchone()
            assert row is not None
            assert row[0].lower() == "wal", "WAL mode should be enabled"

            db.close()
        finally:
            shutil.rmtree(tmpdir)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
