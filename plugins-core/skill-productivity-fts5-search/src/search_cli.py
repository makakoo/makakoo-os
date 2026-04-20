#!/usr/bin/env python3
"""
FTS5 Conversation Search CLI

Usage:
    harvey search "query"                    # Basic search
    harvey search "query" --limit 5         # Limit results
    harvey search "query" --since 2026-03-01 # Filter by date
    harvey search --stats                    # Show index stats
    harvey search --reindex                  # Re-index all journals
"""

import argparse
import sys
from pathlib import Path

# Add skills directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from search import search, get_index_stats
from indexer import index_all


def format_result(r, idx):
    date_short = f"{r.date[5:7]}/{r.date[8:10]}"
    snippet = r.snippet or r.content[:120]
    print(f"  {idx}. [{date_short} {r.timestamp}] {r.session_id[-3:]}")
    print(f"     {snippet[:120]}")
    if r.score:
        print(f"     score: {r.score:.3f}")
    print()


def main():
    parser = argparse.ArgumentParser(prog="harvey search", description="Search Harvey conversation history")
    parser.add_argument("query", nargs="?", help="Search query")
    parser.add_argument("--limit", type=int, default=10, help="Max results (default 10)")
    parser.add_argument("--since", help="Since date (YYYY-MM-DD)")
    parser.add_argument("--until", help="Until date (YYYY-MM-DD)")
    parser.add_argument("--stats", action="store_true", help="Show index statistics")
    parser.add_argument("--reindex", action="store_true", help="Re-index all journals")
    parser.add_argument("--session", help="Filter by session ID")

    args = parser.parse_args()

    if args.stats:
        stats = get_index_stats()
        if "error" in stats:
            print(f"Index not initialized. Run --reindex first.")
        else:
            print(f"\n{'='*60}")
            print(f" FTS5 Conversation Index")
            print(f"{'='*60}")
            print(f"  Total entries:  {stats['total_entries']}")
            print(f"  Days indexed:  {stats['days_indexed']}")
            print(f"  Sessions:      {stats['sessions']}")
            print(f"{'='*60}\n")
        return

    if args.reindex:
        print("Re-indexing all journals...")
        result = index_all()
        print(f"Done: {result['entries_indexed']} new entries from {result['journals_processed']} journals")
        return

    if not args.query:
        parser.print_help()
        return

    results = search(
        args.query,
        limit=args.limit,
        date_from=args.since,
        date_to=args.until,
        session_id=args.session,
    )

    if not results:
        print(f"No results for: {args.query}")
        return

    print(f"\n{'='*60}")
    print(f" Search: {args.query} ({len(results)} results)")
    print(f"{'='*60}")

    for i, r in enumerate(results, 1):
        format_result(r, i)

    print(f"{'='*60}")
    print(f" Run `harvey search --stats` for index stats")


if __name__ == "__main__":
    main()
