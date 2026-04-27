#!/usr/bin/env python3
"""
Test script for Memory Retrieval System.

This script:
1. Creates/Resumes a session
2. Loads session context via MemoryLoader
3. Prints the context to verify functionality
"""

import os
import sys

# Add skill directory to path
from pathlib import Path
SKILL_DIR = str(Path(__file__).resolve().parent)
sys.path.insert(0, SKILL_DIR)

from memory_loader import MemoryLoader, LAYER_CONFIGS
from memory_scorer import MemoryScorer, filter_and_rank
from memory_summarizer import MemorySummarizer
from session_state import SessionStateManager
from freshness_validator import MemoryFreshnessValidator, FreshnessLevel
from proactive_injector import ProactiveContextInjector


def test_session_state():
    """Test 1: Session State Manager"""
    print("=" * 60)
    print("TEST 1: Session State Manager")
    print("=" * 60)

    mgr = SessionStateManager()
    session = mgr.get_or_create_session()

    print(f"Session ID: {session['session_id']}")
    print(f"Started: {session['started_at']}")
    print(f"Git Branch: {session['git_branch']}")
    print(f"Directory: {session['directory']}")

    # Update with test data
    mgr.update_state(session['session_id'], {
        "context_summary": "Testing memory retrieval system",
        "active_files": [__file__],
        "pending_tasks": ["Complete memory retrieval test"]
    })

    # Verify update
    updated = mgr._load_state(session['session_id'])
    print(f"Context updated: {updated['context_summary']}")

    print("\n[PASS] Session State Manager working\n")
    return session['session_id']


def test_memory_loader():
    """Test 2: Memory Loader"""
    print("=" * 60)
    print("TEST 2: Memory Loader (6-layer retrieval)")
    print("=" * 60)

    loader = MemoryLoader()
    print(f"Layer configs available: {list(LAYER_CONFIGS.keys())}")

    # Load with limited tokens to test budget management
    context = loader.load_session_context(available_tokens=20000)

    print(f"\nContext length: {len(context)} chars")
    print(f"Context preview (first 1000 chars):\n")
    print(context[:1000] if context else "No context loaded")

    print("\n[PASS] Memory Loader working\n")
    return context


def test_memory_scorer():
    """Test 3: Memory Scorer"""
    print("=" * 60)
    print("TEST 3: Memory Scorer")
    print("=" * 60)

    test_memories = [
        {
            "title": "Career Lead - Acme Corp",
            "content": "Applied for senior engineer role. Interview scheduled for next week.",
            "tags": ["career", "interview", "acme"],
            "last_interaction": "2026-03-25T10:00:00Z",
            "access_count": 5
        },
        {
            "title": "Harvey OS - Memory System",
            "content": "Implementing pre-session memory loading for Harvey OS.",
            "tags": ["harvey-os", "project", "memory"],
            "last_interaction": "2026-03-27T09:00:00Z",
            "access_count": 12
        },
        {
            "title": "Old Note",
            "content": "Some old note from months ago.",
            "tags": ["misc"],
            "last_interaction": "2025-06-01T00:00:00Z",
            "access_count": 1
        }
    ]

    scorer = MemoryScorer()

    # Test with career query
    print("\nQuery: 'career interview'")
    ranked = scorer.score_memories(test_memories, "career interview")
    for mem in ranked:
        print(f"  [{mem['relevance_score']:.2f}] {mem['title']}")

    # Test with Harvey query
    print("\nQuery: 'Harvey OS memory'")
    ranked = scorer.score_memories(test_memories, "Harvey OS memory")
    for mem in ranked:
        print(f"  [{mem['relevance_score']:.2f}] {mem['title']}")

    print("\n[PASS] Memory Scorer working\n")


def test_memory_summarizer():
    """Test 4: Memory Summarizer"""
    print("=" * 60)
    print("TEST 4: Memory Summarizer")
    print("=" * 60)

    summarizer = MemorySummarizer()

    # Test short text (no compression)
    short_text = "This is a short note about a career lead."
    result = summarizer.summarize(short_text)
    print(f"Short text ({len(short_text)} chars): {result}")

    # Test long text (compression)
    long_text = "This is a much longer piece of text. " * 50
    print(f"\nLong text ({len(long_text)} chars) compression level: {summarizer.get_compression_level(long_text)}")
    result = summarizer.summarize(long_text, max_words=50)
    print(f"Summarized: {result[:200]}...")

    print("\n[PASS] Memory Summarizer working\n")


def test_freshness_validator():
    """Test 5: Freshness Validator"""
    print("=" * 60)
    print("TEST 5: Freshness Validator")
    print("=" * 60)

    validator = MemoryFreshnessValidator()

    test_memories = [
        {"title": "Recent", "last_interaction": "2026-03-27T10:00:00Z"},
        {"title": "WeekOld", "last_interaction": "2026-03-20T10:00:00Z"},
        {"title": "MonthOld", "last_interaction": "2026-02-27T10:00:00Z"},
        {"title": "Old", "last_interaction": "2025-06-01T00:00:00Z"},
        {"title": "NoDate", "last_interaction": None},
    ]

    for mem in test_memories:
        level = validator.check_freshness(mem)
        print(f"  {mem['title']}: {level}")

    # Get freshness report
    report = validator.get_freshness_report()
    print(f"\nFreshness report: {report}")

    # Flag stale leads
    stale_leads = validator.flag_stale_leads(threshold_days=30)
    print(f"\nStale leads (>30 days): {len(stale_leads)}")

    print("\n[PASS] Freshness Validator working\n")


def test_proactive_injector():
    """Test 6: Proactive Context Injector"""
    print("=" * 60)
    print("TEST 6: Proactive Context Injector")
    print("=" * 60)

    injector = ProactiveContextInjector()
    injections = injector.get_proactive_injections()

    print(f"Injections found: {len(injections)}")
    for inj in injections:
        print(f"  [{inj['priority']}] {inj['type']}: {inj['content']}")

    print("\n[PASS] Proactive Context Injector working\n")


def main():
    print("\n" + "=" * 60)
    print("MEMORY RETRIEVAL SYSTEM - INTEGRATION TEST")
    print("=" * 60 + "\n")

    session_id = None

    try:
        session_id = test_session_state()
        test_memory_loader()
        test_memory_scorer()
        test_memory_summarizer()
        test_freshness_validator()
        test_proactive_injector()

        print("=" * 60)
        print("ALL TESTS PASSED")
        print("=" * 60)

        # End the test session
        if session_id:
            mgr = SessionStateManager()
            mgr.end_session(session_id)
            print(f"\nSession {session_id} ended successfully.")

    except Exception as e:
        print(f"\n[FAIL] Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
