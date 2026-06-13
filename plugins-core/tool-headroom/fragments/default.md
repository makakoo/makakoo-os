<!-- makakoo:fragment:headroom -->
## Headroom — tool/MCP context compression (default available)

Makakoo fresh installs include Headroom (`headroom-ai[mcp]`) for large tool outputs and MCP retrieval. When the host exposes Headroom MCP tools, use them for bulky logs, JSON, diffs, grep output, web text, and repeated structural data:

- `headroom_compress` — compress noisy bulk output and keep a retrieval marker.
- `headroom_retrieve` — fetch exact original content when details matter.
- `headroom_stats` — inspect savings/context pressure.

Hard boundaries: never compress secrets, destructive confirmations, safety-critical commands, exact patches, or final publish/send prose. Preserve exact error lines, commands, paths, hashes, versions, and IDs before compressing surrounding noise.

If Headroom MCP is missing, run `headroom mcp install` after installing `headroom-ai[mcp]`, then restart the CLI session.
<!-- makakoo:fragment:headroom-end -->
