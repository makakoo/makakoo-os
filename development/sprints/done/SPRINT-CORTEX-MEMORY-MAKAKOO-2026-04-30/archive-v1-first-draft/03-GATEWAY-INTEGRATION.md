# 03 — Gateway Integration

## Changes to `core/chat/gateway.py`

### 1. Import Cortex memory

```python
from core.cortex import get_cortex_memory
```

### 2. Add Cortex to `HarveyChat.__init__`

```python
class HarveyChat:
    def __init__(self, config: ChatConfig = None):
        # ... existing init ...
        self.cortex = get_cortex_memory(self.store.db_path)  # None if not enabled
        if self.cortex:
            log.info("[gateway] Cortex memory enabled")
        else:
            log.info("[gateway] Cortex memory disabled")
```

### 3. Modify `handle_message()`

```python
async def handle_message(self, channel: str, user_id: str, username: str, text: str) -> str:
    # ... existing command handling (/status, /clear, etc.) ...
    
    # Store user message in ChatStore (always)
    self.store.add_message(channel, user_id, "user", text, {"username": username})
    
    # NEW: Cortex session + message tracking
    cortex_session_id = None
    if self.cortex:
        try:
            cortex_session_id = self.cortex.get_or_create_session(channel, user_id)
            self.cortex.add_message(cortex_session_id, "user", text)
        except Exception as e:
            log.warning("Cortex user message tracking failed: %s", e)
            cortex_session_id = None
    
    # ... rest of handle_message ...
    
    # Get conversation history (from ChatStore, unchanged)
    history = self.store.get_history(channel, user_id, limit=self.config.bridge.max_history_messages)
    
    # NEW: Query Cortex for memories if available
    memories = []
    if self.cortex and cortex_session_id:
        try:
            memories = self.cortex.search(text, channel, user_id, limit=self.config.cortex.memory_limit)
        except Exception as e:
            log.warning("Cortex memory search failed: %s", e)
    
    # Route to bridge with memories
    response = self.bridge.send(
        text,
        history,
        channel,
        memories=memories,  # NEW parameter
        task_id=cognitive_task_id,
        store=self.task_store,
    )
    
    # Sanitize response
    response = self._sanitize_response(response)
    
    # Store assistant response in ChatStore (always)
    self.store.add_message(
        channel, user_id, "assistant", response,
        reasoning_content=getattr(self.bridge, "_last_reasoning_content", None),
    )
    
    # NEW: Track assistant response in Cortex
    if self.cortex and cortex_session_id:
        try:
            self.cortex.add_message(cortex_session_id, "assistant", response)
            # Auto-create memory from assistant response if it contains facts
            if len(response) > 50:
                self.cortex.create_memory(
                    content=response[:500],
                    channel=channel,
                    user_id=user_id,
                    importance=0.5,
                )
            # Auto-summarize after N messages
            msg_count = self.cortex.get_session_message_count(cortex_session_id)
            if msg_count % self.config.cortex.auto_summarize_after == 0:
                self._trigger_summarization(cortex_session_id)
        except Exception as e:
            log.warning("Cortex assistant tracking failed: %s", e)
    
    # Log to Brain
    if self.config.log_to_brain and _is_significant(text, response):
        log_to_journal(channel, f"@{username}: {text[:80]}", response[:200])
    
    return response
```

### 4. Handle `/clear` with Cortex

```python
if text == "/clear":
    # Clear ChatStore
    self.store.clear_history(channel, user_id)
    
    # NEW: Reset Cortex session for this user
    if self.cortex:
        try:
            key = f"{channel}:{user_id}"
            if key in self.cortex._session_cache:
                del self.cortex._session_cache[key]
            # Create new session on next message
        except Exception as e:
            log.warning("Cortex clear failed: %s", e)
    
    return "Context cleared. Starting fresh."
```

### 5. Summarization helper

```python
def _trigger_summarization(self, session_id: str) -> None:
    """Async summarization of session."""
    import asyncio
    from core.cortex.summarizer import summarize_session
    
    # Get recent messages from ChatStore for this session
    # (session tracks metadata, messages are in ChatStore)
    # For now, skip — implement when needed
    pass
```

### 6. No changes to channel adapters

DiscordChannel, TelegramChannel, etc. — **unchanged**. They still call `gateway.handle_message()` with `(channel, user_id, username, text)`. The gateway handles all Cortex logic internally.

## Changes to `core/chat/config.py`

Add `CortexConfig` to `ChatConfig`:

```python
@dataclass
class ChatConfig:
    telegram: TelegramConfig = field(default_factory=TelegramConfig)
    discord: DiscordConfig = field(default_factory=DiscordConfig)
    bridge: BridgeConfig = field(default_factory=BridgeConfig)
    cortex: CortexConfig = field(default_factory=CortexConfig)  # NEW
    db_path: str = "data/chat/store.db"
    pid_file: str = "data/chat/harveychat.pid"
    log_to_brain: bool = True
    brain_sync_interval: int = 300
```

And update `load_config()` to read the `cortex` section from `config.json`.

## Minimal config.json addition

```json
{
  "telegram": { ... },
  "discord": { ... },
  "bridge": { ... },
  "cortex": {
    "enabled": true,
    "memory_limit": 5,
    "auto_summarize_after": 4,
    "pii_scrubbing": true
  }
}
```
