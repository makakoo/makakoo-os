# 04 — Bridge Integration

## Changes to `core/chat/bridge.py`

### 1. Add `memories` parameter to `send()`

```python
def send(
    self,
    message: str,
    history: List[Dict],
    channel: str = "telegram",
    file_sender: Optional[Callable[[str, str], None]] = None,
    task_id: Optional[str] = None,
    store: Optional[Any] = None,
    memories: Optional[List[Dict]] = None,  # NEW
) -> str:
```

### 2. Build enriched system prompt with memories

```python
def _build_system_prompt(self, channel: str = "telegram", memories: Optional[List[Dict]] = None) -> str:
    """Build tool-aware system prompt (grants refreshed every turn) + Cortex memories."""
    base_prompt = render_system_prompt(channel=channel)
    
    if memories:
        memory_block = self._format_memories(memories)
        if memory_block:
            base_prompt += f"\n\n{memory_block}"
    
    return base_prompt
```

### 3. Memory formatting

```python
def _format_memories(self, memories: List[Dict]) -> str:
    """Format Cortex memories for injection into system prompt.
    
    Memories from Cortex look like:
    [
      {"content": "User decided to migrate to DigitalOcean", "importance_score": 0.85, "created_at": "2026-04-22T..."},
      {"content": "User prefers concise responses", "importance_score": 0.7, "created_at": "2026-04-20T..."},
    ]
    """
    if not memories:
        return ""
    
    lines = ["## Relevant Context from Past Conversations"]
    for m in memories:
        content = m.get("content", "")
        if not content:
            continue
        # Truncate long memories
        if len(content) > 200:
            content = content[:197] + "..."
        lines.append(f"- {content}")
    
    return "\n".join(lines)
```

### 4. Use enriched prompt in agent call

```python
# In send(), before calling agent.process():
system_prompt = self._build_system_prompt(channel, memories=memories)
```

The rest of `send()` is unchanged — the agent gets the same message, history, and system prompt. The only difference is the prompt now includes a "Relevant Context from Past Conversations" block.

### 5. Memory contamination guard

Memories are injected as a **system prompt block**, not as fake assistant messages. This prevents the LLM from learning false conversation patterns.

```
System prompt (base)
---
## Relevant Context from Past Conversations
- User decided to migrate to DigitalOcean on April 22
- User prefers concise responses
---
History messages...
User: What about the migration?
```

## Example: Before vs After

### Before (no Cortex)

```
System: You are Harvey... [tools]...
History:
  user: hi
  assistant: hey
  user: What about the migration?
```

Harvey: "I don't have context about a migration. Can you tell me more?"

### After (with Cortex)

```
System: You are Harvey... [tools]...
## Relevant Context from Past Conversations
- User decided to migrate from AWS to DigitalOcean on April 22
- User asked about Kubernetes cost optimization on April 25
History:
  user: hi
  assistant: hey
  user: What about the migration?
```

Harvey: "You decided to migrate to DigitalOcean on April 22. Last time you asked about K8s cost optimization. Do you want me to check current DigitalOcean pricing or review your migration checklist?"

## Memory freshness

Memories are queried **once per user message**. No caching within a turn — if the user sends a follow-up, we re-query with the new message. This is slightly slower but ensures relevance.

**Optimization:** Cache memory results for 30 seconds within the same session to avoid duplicate queries during rapid back-and-forth.

```python
# In gateway.py
_memory_cache: Dict[str, tuple] = {}  # key: session_id, value: (timestamp, memories)

_CORTEX_MEMORY_CACHE_TTL = 30  # seconds

def _get_cached_memories(self, session_id: str, query: str) -> Optional[List[Dict]]:
    import time
    cached = self._memory_cache.get(session_id)
    if cached:
        ts, memories = cached
        if time.time() - ts < _CORTEX_MEMORY_CACHE_TTL:
            return memories
    return None

def _cache_memories(self, session_id: str, memories: List[Dict]) -> None:
    import time
    self._memory_cache[session_id] = (time.time(), memories)
```
