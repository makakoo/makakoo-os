"""Tests for brain_bridge v0.2 C.1 additions.

Covers:
  - upsert_page_properties(name, props) — dict-based batch upsert
  - search(pattern, limit) — name + body match
  - get_all_pages() — enumerate every page
  - link(from, to, label) — idempotent wikilink append

Each test stubs BRAIN_DIR / PAGES_DIR / JOURNALS_DIR to a tempdir and
reloads the module so in-module paths pick up the override.
"""
from __future__ import annotations

import importlib
import os
import tempfile
import unittest
from pathlib import Path


class BrainBridgeV2Tests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.mkdtemp(prefix="brain_bridge_test_")
        # Force the brain bridge to point at the tempdir by pre-setting
        # env vars it reads on import.
        os.environ["HARVEY_HOME"] = self._tmp
        os.environ["MAKAKOO_HOME"] = self._tmp
        # Ensure a clean reload so module-level paths pick up the env vars.
        import core.memory.brain_bridge as bb
        self.bb = importlib.reload(bb)
        self.pages = Path(self.bb.PAGES_DIR)
        self.pages.mkdir(parents=True, exist_ok=True)

    def tearDown(self) -> None:
        import shutil
        shutil.rmtree(self._tmp, ignore_errors=True)

    def test_upsert_page_properties_creates_page_and_sets_multiple_keys(self):
        ok = self.bb.upsert_page_properties(
            "Lead - Alice",
            {"type": "lead", "score": 87, "status": "Active"},
        )
        self.assertTrue(ok)
        body = (self.pages / "Lead - Alice.md").read_text()
        self.assertIn("type:: lead", body)
        self.assertIn("score:: 87", body)
        self.assertIn("status:: Active", body)

    def test_upsert_page_properties_updates_existing_keys(self):
        self.bb.upsert_page_properties("Foo", {"status": "Active"})
        self.bb.upsert_page_properties("Foo", {"status": "Sent", "added": "true"})
        body = (self.pages / "Foo.md").read_text()
        self.assertIn("status:: Sent", body)
        self.assertIn("added:: true", body)
        self.assertNotIn("status:: Active", body)

    def test_search_matches_page_name_case_insensitive(self):
        self.bb.create_page("Alpha Bravo", {"type": "page"}, "hello world")
        self.bb.create_page("Charlie", {"type": "page"}, "nothing to see")
        hits = self.bb.search("alpha")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["name"], "Alpha Bravo")
        self.assertEqual(hits[0]["match"], "name")

    def test_search_matches_body_as_fallback(self):
        self.bb.create_page("OnlyBody", {"type": "page"}, "mysterious keyword here")
        self.bb.create_page("Noise", {"type": "page"}, "unrelated")
        hits = self.bb.search("mysterious")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["name"], "OnlyBody")
        self.assertEqual(hits[0]["match"], "body")

    def test_search_respects_limit(self):
        for i in range(5):
            self.bb.create_page(f"Match{i}", {"type": "page"}, "")
        hits = self.bb.search("Match", limit=3)
        self.assertEqual(len(hits), 3)

    def test_search_empty_pattern_returns_empty(self):
        self.assertEqual(self.bb.search(""), [])

    def test_get_all_pages_lists_every_markdown_page(self):
        self.bb.create_page("A", {}, "")
        self.bb.create_page("B", {}, "")
        self.bb.create_page("C", {}, "")
        pages = self.bb.get_all_pages()
        names = sorted(p["name"] for p in pages)
        self.assertEqual(names, ["A", "B", "C"])

    def test_get_all_pages_empty_graph_returns_empty(self):
        # Pages dir exists but is empty.
        self.assertEqual(self.bb.get_all_pages(), [])

    def test_link_appends_wikilink_bullet(self):
        self.bb.create_page("Alice", {"type": "person"}, "")
        self.assertTrue(self.bb.link("Alice", "ACME Corp"))
        body = (self.pages / "Alice.md").read_text()
        self.assertIn("- [[ACME Corp]]", body)

    def test_link_is_idempotent(self):
        self.bb.link("Alice", "ACME Corp")
        self.bb.link("Alice", "ACME Corp")
        body = (self.pages / "Alice.md").read_text()
        # Exactly one occurrence of the bullet.
        self.assertEqual(body.count("- [[ACME Corp]]"), 1)

    def test_link_with_label_prepends_prefix(self):
        self.bb.link("Alice", "ACME Corp", label="employed-by::")
        body = (self.pages / "Alice.md").read_text()
        self.assertIn("employed-by:: [[ACME Corp]]", body)


if __name__ == "__main__":
    unittest.main()
