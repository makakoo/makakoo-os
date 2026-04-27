# Example: memory query → answer with source

User says:

> "What did I decide about the auth middleware last month?"

## What you do

### 1. Pick the right tool

This is a *question* asking for an *answer*, not a request for raw
hits. Use `harvey_superbrain_query` (FTS + vector + entity graph +
LLM synthesis):

```
mcp__harvey__harvey_superbrain_query({
  question: "What did I decide about the auth middleware last month?"
})
```

### 2. Present the answer

The tool returns text like:

```
The auth middleware was rewritten on 2026-03-15 to use httpOnly
cookies with 15min expiry + refresh rotation, replacing the prior
localStorage approach. The change was driven by the CSO audit which
flagged token storage as non-compliant with the new requirements.

Sources:
  - [journal] 2026_03_15.md
  - [page] auth-middleware.md
  - [page] CSO-audit-2026.md
```

Quote the answer to the user *with* the source list. Don't strip the
sources — they're how the user verifies you didn't hallucinate.

### 3. If they want to dig deeper

Offer the brain_search follow-up: "Want the raw journal entries?"
Then call:

```
mcp__harvey__harvey_brain_search({
  query: "auth middleware",
  top_k: 10
})
```

## Anti-pattern

Don't do this:

```
You: "I think you decided to use JWT validation? But I'm not sure."
```

You guessed instead of searching. Always call a brain tool first
when the user asks about their own history.
