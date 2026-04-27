"""D.4 — Python producer ↔ Rust consumer compatibility tests.

The Rust SwarmDispatchHandler reads the queue that this Python module
writes. Because they ship in separate binaries, the only contract we
have is the JSONL schema — a mismatch means lost dispatches with no
loud error. These tests lock the on-disk shape.
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

_SRC = Path(__file__).resolve().parents[3]
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from core.orchestration.swarm_dispatch_queue import (  # noqa: E402
    enqueue_agent,
    enqueue_team,
    queue_depth,
    queue_path,
    receipts_path,
)


class SwarmDispatchQueueTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.home = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def _read_queue(self):
        qp = queue_path(self.home)
        if not qp.exists():
            return []
        with open(qp) as f:
            return [json.loads(line) for line in f if line.strip()]

    def test_enqueue_team_writes_rust_compatible_jsonl(self):
        qid = enqueue_team(
            "research_team",
            "what is lope?",
            parallelism=4,
            model="ail-compound",
            home=self.home,
        )
        rows = self._read_queue()
        self.assertEqual(len(rows), 1)
        row = rows[0]
        # Discriminator must match the Rust serde tag.
        self.assertEqual(row["kind"], "team")
        self.assertEqual(row["id"], qid)
        self.assertEqual(row["team"], "research_team")
        self.assertEqual(row["prompt"], "what is lope?")
        self.assertEqual(row["parallelism"], 4)
        self.assertEqual(row["model"], "ail-compound")
        # Timestamp shape compatible with chrono's DateTime<Utc> parser.
        self.assertTrue(row["enqueued_at"].endswith("Z"))

    def test_enqueue_agent_writes_rust_compatible_jsonl(self):
        qid = enqueue_agent(
            "researcher",
            "lookup",
            "tell me about lope",
            model="ail-compound",
            home=self.home,
        )
        rows = self._read_queue()
        self.assertEqual(len(rows), 1)
        row = rows[0]
        self.assertEqual(row["kind"], "agent")
        self.assertEqual(row["id"], qid)
        self.assertEqual(row["name"], "researcher")
        self.assertEqual(row["task"], "lookup")
        self.assertEqual(row["prompt"], "tell me about lope")
        self.assertEqual(row["model"], "ail-compound")
        self.assertNotIn("parent_run_id", row, "optional fields must be omitted, not null")

    def test_queue_depth_masks_receipted_entries(self):
        id1 = enqueue_team("research_team", "a", home=self.home)
        id2 = enqueue_team("archive_team", "b", home=self.home)
        self.assertEqual(queue_depth(self.home), 2)

        # Write a receipt for id1 by hand — the Rust side writes these,
        # but the Python reader has to honor them too.
        rp = receipts_path(self.home)
        rp.parent.mkdir(parents=True, exist_ok=True)
        with open(rp, "w") as f:
            f.write(json.dumps({
                "id": id1,
                "dispatched_at": "2026-04-21T00:00:00.000000Z",
                "run_id": "already-run",
            }) + "\n")

        self.assertEqual(queue_depth(self.home), 1)
        # id2 is still pending.
        remaining = [r["id"] for r in self._read_queue()]
        self.assertIn(id2, remaining)

    def test_ids_are_unique_under_rapid_calls(self):
        ids = {enqueue_team("research_team", f"prompt {i}", home=self.home) for i in range(30)}
        self.assertEqual(len(ids), 30, "duplicate ids minted under rapid enqueue")

    def test_empty_home_has_depth_zero(self):
        self.assertEqual(queue_depth(self.home), 0)


if __name__ == "__main__":
    unittest.main()
