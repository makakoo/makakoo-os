#!/usr/bin/env python3
"""Test embedding service"""
import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from embedding_service import EmbeddingService

print("Testing EmbeddingService...")

svc = EmbeddingService.get_instance()
print(f"Service initialized: {svc}")

# Test semantic search
print("\nTest 1: Search for 'review pull request and check emails'")
results = svc.search_skills("I want to review a pull request and check emails", top_k=5)
for r in results:
    print(f"  {r['name']}: {r['score']:.3f}")
    print(f"    Category: {r['category']}")
    print(f"    Path: {r['path']}")

# Test another query
print("\nTest 2: Search for 'deploy to production'")
results = svc.search_skills("deploy to production", top_k=5)
for r in results:
    print(f"  {r['name']}: {r['score']:.3f}")

# Test another query
print("\nTest 3: Search for 'send email to someone'")
results = svc.search_skills("send email to someone", top_k=5)
for r in results:
    print(f"  {r['name']}: {r['score']:.3f}")

print("\nAll tests completed!")
