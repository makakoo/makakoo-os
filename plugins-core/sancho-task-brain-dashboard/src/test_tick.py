"""C.7 — brain dashboard smoke tests."""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

_HERE = Path(__file__).resolve().parent
if str(_HERE) not in sys.path:
    sys.path.insert(0, str(_HERE))

import tick  # noqa: E402


class BrainDashboardTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.home = Path(self._tmp.name)
        (self.home / "data" / "Brain" / "journals").mkdir(parents=True)
        (self.home / "data" / "Brain" / "pages").mkdir(parents=True)
        self._saved_env = {
            "MAKAKOO_HOME": os.environ.pop("MAKAKOO_HOME", None),
            "HARVEY_HOME": os.environ.pop("HARVEY_HOME", None),
        }
        os.environ["MAKAKOO_HOME"] = str(self.home)

    def tearDown(self):
        for k, v in self._saved_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        self._tmp.cleanup()

    def _seed_journals(self):
        today = datetime.now(timezone.utc).strftime("%Y_%m_%d")
        (self.home / "data" / "Brain" / "journals" / f"{today}.md").write_text(
            "- test entry about [[lope]] and [[makakoo]]\n"
            "- another mentioning [[lope]]\n",
            encoding="utf-8",
        )

    def _seed_pages(self):
        pages = self.home / "data" / "Brain" / "pages"
        (pages / "Alpha.md").write_text("- [[Beta]] linked here", encoding="utf-8")
        (pages / "Beta.md").write_text("- [[Alpha]] linked back", encoding="utf-8")
        (pages / "zebra.md").write_text("- orphan page", encoding="utf-8")

    def test_tick_generates_three_pages(self):
        self._seed_journals()
        self._seed_pages()
        rc = tick.main([])
        self.assertEqual(rc, 0)
        pages = self.home / "data" / "Brain" / "pages"
        for expected in ["index-journals.md", "index-pages.md", "index-god-nodes.md"]:
            self.assertTrue((pages / expected).exists(), f"missing {expected}")

    def test_god_nodes_page_ranks_by_count(self):
        self._seed_journals()
        self._seed_pages()
        tick.main([])
        body = (self.home / "data" / "Brain" / "pages" / "index-god-nodes.md").read_text()
        # `lope` appears twice in the journal, `makakoo` once, so lope should rank higher.
        lope_idx = body.find("[[lope]]")
        makakoo_idx = body.find("[[makakoo]]")
        self.assertGreater(lope_idx, 0)
        self.assertGreater(makakoo_idx, 0)
        self.assertLess(lope_idx, makakoo_idx, "lope must outrank makakoo")

    def test_pages_index_excludes_self(self):
        """Regression: the dashboard writes index-*.md pages. If it
        included them in its own page index we'd get ever-growing
        self-referential bloat."""
        self._seed_pages()
        tick.main([])
        # Second tick — once index-* pages exist on disk, re-running
        # must NOT list them.
        tick.main([])
        body = (self.home / "data" / "Brain" / "pages" / "index-pages.md").read_text()
        self.assertNotIn("[[index-journals]]", body)
        self.assertNotIn("[[index-pages]]", body)
        self.assertNotIn("[[index-god-nodes]]", body)

    def test_empty_home_produces_graceful_pages(self):
        # Fresh install — no journals, no pages beyond the dashboard.
        rc = tick.main([])
        self.assertEqual(rc, 0)
        body = (self.home / "data" / "Brain" / "pages" / "index-journals.md").read_text()
        self.assertIn("no journal entries", body)

    def test_dry_run_writes_nothing(self):
        self._seed_journals()
        self._seed_pages()
        rc = tick.main(["--dry-run"])
        self.assertEqual(rc, 0)
        self.assertFalse(
            (self.home / "data" / "Brain" / "pages" / "index-journals.md").exists(),
            "dry-run must not write",
        )


if __name__ == "__main__":
    unittest.main()
