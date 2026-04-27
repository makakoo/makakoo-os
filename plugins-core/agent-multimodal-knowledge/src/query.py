#!/usr/bin/env python3
"""Query the multimodal knowledge base via Qdrant."""
import os, sys, json, requests
import numpy as np
from google import genai
from google.genai import types

GEMINI_API_KEY = os.getenv("GEMINI_API_KEY")
QDRANT_URL = os.getenv("QDRANT_URL", "http://localhost:6333")
COLLECTION = "multimodal"
client = genai.Client(api_key=GEMINI_API_KEY)

def query(question, top_k=5, content_type=None):
    # Embed query
    resp = client.models.embed_content(
        model="gemini-embedding-2-preview",
        contents=question,
        config=types.EmbedContentConfig(task_type="RETRIEVAL_QUERY"),
    )
    vec = [float(v) for v in resp.embeddings[0].values]

    # Search Qdrant
    payload_filter = {}
    if content_type:
        payload_filter["content_type"] = content_type

    search_body = {
        "vector": vec,
        "limit": top_k,
        "with_payload": True
    }
    if payload_filter:
        search_body["filter"] = {"must": [{"key": "content_type", "match": {"value": content_type}}]}

    r = requests.post(f"{QDRANT_URL}/collections/{COLLECTION}/points/search", json=search_body)
    results = r.json()['result']

    print(f"\n=== RESULTS for: \"{question}\" ===\n")
    for hit in results:
        p = hit['payload']
        print(f"[{hit['score']:.4f}] {p['content_type'].upper()} | {p['title']}")
        print(f"  {(p.get('text_content') or '')[:200]}...")
        if p.get('metadata', {}).get('chunk_info'):
            print(f"  {p['metadata']['chunk_info']}")
        print()
    return results

if __name__ == "__main__":
    q = sys.argv[1] if len(sys.argv) > 1 else "What is this about?"
    t = sys.argv[2] if len(sys.argv) > 2 else None
    query(q, content_type=t)
