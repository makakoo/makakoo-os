"""Mascots package — nursery creatures as useful SANCHO patrols.

The `nursery.json` roster holds 4 collectible mascots with distinct stat
profiles and personalities. This package turns each mascot's highest stat
into a small, useful, scheduled chore that catches real quality issues in
Makakoo OS:

- **Pixel**   (DEBUGGING=53, SNARK=17, "hilariously dramatic") —
  scans recent daemon log tails for fresh tracebacks and error bursts.
  Prevents silent daemon outages from hiding in logs nobody reads.

- **Cinder**  (DEBUGGING=65, "relentlessly optimistic, afraid of semicolons") —
  runs `python -m py_compile` on every .py file changed in the last 24h
  so syntax typos get caught before they land in a production daemon.

- **Ziggy**   (SNARK=60, WISDOM=19, "hoards documentation") —
  uses ast to find public functions/classes without docstrings in any
  .py file changed in the last 24h. Produces a snarky but accurate report.

- **Glimmer** (PATIENCE=56, "deeply curious") —
  patrols `data/Brain/pages/` for stale TODO / FIXME / ??? / XXX markers
  older than 14 days so half-finished notes don't rot in the outliner.

All four patrols are cheap, read-only, and safe to run on every SANCHO
tick. They return JSON-serializable dicts that slot into existing
handler output without any schema changes.

See `patrol.py` for the implementation and `tests/test_mascot_patrol.py`
for coverage.
"""

from .patrol import (
    MascotFinding,
    cinder_syntax_check,
    glimmer_stale_todos,
    patrol_all,
    pixel_daemon_scan,
    ziggy_doc_patrol,
)

__all__ = [
    "MascotFinding",
    "cinder_syntax_check",
    "glimmer_stale_todos",
    "patrol_all",
    "pixel_daemon_scan",
    "ziggy_doc_patrol",
]
