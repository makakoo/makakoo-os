"""
bench_query_anchored — measure token savings of anchored vs classic query path.

Runs the same set of test queries through `Superbrain.query()` with the
classic full-content path (BRAIN_USE_ANCHORS=0) and with the anchored
path (BRAIN_USE_ANCHORS=1 → delegates to query_anchored). Compares:

  - total source chars returned (proxy for LLM context tokens)
  - number of sources
  - query time
  - synthesize prompt length (if synthesize=True)

No synthesis is run by default — that would burn LLM tokens for the
measurement itself. Pass --synthesize to enable and compare synthesized
answers as well.

Usage:
    python -m core.superbrain.bench_query_anchored
    python -m core.superbrain.bench_query_anchored --queries q1 q2 q3
    python -m core.superbrain.bench_query_anchored --synthesize --topk 10
    python -m core.superbrain.bench_query_anchored --json

All measurements are approximate — char count / 4 ≈ token count for
most Latin-script English prose.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

_THIS = Path(__file__).resolve()
sys.path.insert(0, str(_THIS.parent.parent.parent))

from core.superbrain.superbrain import Superbrain  # noqa: E402

DEFAULT_QUERIES = [
    "linkedin job application lead",
    "KRIMDA INTELCIA",
    "AI architect freelance match score",
    "career manager pipeline status",
    "LinkedIn application Sebastian",
]


def _chars_to_tokens(n: int) -> int:
    return n // 4


def _measure(sb: Superbrain, question: str, top_k: int, synthesize: bool,
             use_anchors: bool) -> dict:
    os.environ["BRAIN_USE_ANCHORS"] = "1" if use_anchors else "0"
    t0 = time.time()
    if use_anchors:
        result = sb.query_anchored(
            question,
            top_k=top_k,
            synthesize=synthesize,
            refine_steps=1,
            expand_top_n=2,
        )
    else:
        # Classic path — call the internal code directly, bypassing the gate,
        # by calling query() with BRAIN_USE_ANCHORS=0 which forces the
        # classic branch.
        result = sb.query(
            question,
            top_k=top_k,
            synthesize=synthesize,
            refine_steps=1,
        )
    elapsed = time.time() - t0

    total_chars = sum(len(s.text or "") for s in result.sources)
    titles = [s.title for s in result.sources[:5]]
    expanded_ct = sum(1 for s in result.sources if s.metadata.get("expanded"))
    return {
        "question": question,
        "path": "anchored" if use_anchors else "classic",
        "sources": len(result.sources),
        "total_source_chars": total_chars,
        "approx_source_tokens": _chars_to_tokens(total_chars),
        "elapsed_sec": round(elapsed, 2),
        "systems_queried": result.systems_queried,
        "top_titles": titles,
        "auto_expanded": expanded_ct,
        "answer_len": len(result.answer or ""),
    }


def _format_diff(classic: dict, anchored: dict) -> str:
    def pct(new: int, old: int) -> str:
        if old == 0:
            return "n/a"
        delta = (new - old) / old * 100
        sign = "+" if delta >= 0 else ""
        return f"{sign}{delta:.1f}%"

    lines = []
    lines.append(f"  query: {classic['question']!r}")
    lines.append(f"    classic   sources={classic['sources']:<3} chars={classic['total_source_chars']:<6} "
                 f"≈tokens={classic['approx_source_tokens']:<5} elapsed={classic['elapsed_sec']}s")
    lines.append(f"    anchored  sources={anchored['sources']:<3} chars={anchored['total_source_chars']:<6} "
                 f"≈tokens={anchored['approx_source_tokens']:<5} elapsed={anchored['elapsed_sec']}s")
    ct_c = classic["total_source_chars"]
    ct_a = anchored["total_source_chars"]
    lines.append(f"    savings   chars {pct(ct_a, ct_c):<8} "
                 f"(absolute: {ct_c - ct_a} chars ≈ {_chars_to_tokens(ct_c - ct_a)} tokens)")
    lines.append(f"    expanded  classic=N/A anchored={anchored['auto_expanded']} (auto)")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--queries", nargs="+", default=DEFAULT_QUERIES)
    ap.add_argument("--topk", type=int, default=10)
    ap.add_argument("--synthesize", action="store_true",
                    help="run real synthesis (burns LLM tokens)")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    sb = Superbrain()
    results = []
    total_classic_chars = 0
    total_anchored_chars = 0

    for q in args.queries:
        classic = _measure(sb, q, args.topk, args.synthesize, use_anchors=False)
        anchored = _measure(sb, q, args.topk, args.synthesize, use_anchors=True)
        results.append({"classic": classic, "anchored": anchored})
        total_classic_chars += classic["total_source_chars"]
        total_anchored_chars += anchored["total_source_chars"]

    if args.json:
        print(json.dumps({
            "queries": results,
            "totals": {
                "classic_chars": total_classic_chars,
                "anchored_chars": total_anchored_chars,
                "savings_chars": total_classic_chars - total_anchored_chars,
                "savings_pct": round(
                    (1 - total_anchored_chars / total_classic_chars) * 100, 2
                ) if total_classic_chars else 0,
            },
        }, indent=2, default=str))
        return 0

    print("═" * 72)
    print("  query_anchored benchmark — classic vs anchored read path")
    print("═" * 72)
    for r in results:
        print(_format_diff(r["classic"], r["anchored"]))
        print()
    if total_classic_chars > 0:
        savings = 1 - total_anchored_chars / total_classic_chars
        print("─" * 72)
        print(f"  AGGREGATE across {len(results)} queries:")
        print(f"    classic  total_chars = {total_classic_chars} (≈{_chars_to_tokens(total_classic_chars)} tokens)")
        print(f"    anchored total_chars = {total_anchored_chars} (≈{_chars_to_tokens(total_anchored_chars)} tokens)")
        print(f"    SAVINGS = {savings * 100:.1f}% ({total_classic_chars - total_anchored_chars} chars saved)")
        print("═" * 72)

    return 0


if __name__ == "__main__":
    sys.exit(main())
