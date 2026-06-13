---
name: tool-headroom
description: Use Headroom context compression for large tool outputs, long logs, huge file reads, MCP retrieval, and agent sessions where context budget is being burned by generated or tool-returned text. Trigger when output is too large, repeated, or mostly structural, or when MCP exposes headroom_compress/headroom_retrieve/headroom_stats.
---

# Headroom for Makakoo tools

Headroom is Makakoo's default context compression layer for tool and MCP output.

Fresh installs try the Python package first (`headroom-ai[mcp]>=0.25.0`), then fall back to Headroom's Docker-native wrapper when Python wheels/builds are unavailable.

```bash
headroom mcp status
headroom mcp install --agent claude --proxy-url http://127.0.0.1:8787
```

## Use it when

- A tool returns a large log, JSON payload, directory listing, diff, grep result, test report, or copied web text.
- The agent only needs the shape, key failures, IDs, paths, counts, or a compact summary now, with retrieval available later.
- The host exposes `headroom_compress`, `headroom_retrieve`, or `headroom_stats` through the Headroom MCP server.

## Workflow

1. Preserve exact high-signal lines first: errors, commands, file paths, hashes, IDs, versions, timestamps.
2. Compress noisy bulk output with `headroom_compress` when available.
3. Keep the returned retrieval marker/hash in the conversation or notes.
4. Call `headroom_retrieve` if the exact original is needed.
5. Use `headroom_stats` to confirm savings when diagnosing context bloat.

## Do not compress

- Secrets, credentials, private keys, recovery codes, or tokens.
- Destructive-action confirmations or safety-critical instructions.
- Final external prose that the user asked to publish or send.
- Tiny outputs where compression adds complexity.
- Code patches where exact whitespace and context are load-bearing.

If in doubt, quote the exact critical lines and compress only the surrounding noise.
