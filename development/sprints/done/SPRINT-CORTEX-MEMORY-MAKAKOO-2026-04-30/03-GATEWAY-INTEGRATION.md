# 03 — Gateway Integration

## Target file

```text
plugins/lib-harvey-core/src/core/chat/gateway.py
```

## Current flow to respect

Actual code does not call bridge inline from `handle_message()` only. It routes through:

- `handle_message()`
- `_handle_new_message()`
- `_bridge_send_with_file_hints()`
- `bridge.send()` inside executor

Cortex must integrate with that flow, not bypass it.

## Initialization

In `HarveyChat.__init__` after `self.store = ChatStore(...)`:

```python
self.cortex = None
try:
    from core.cortex import get_cortex_memory
    self.cortex = get_cortex_memory(self.store.db_path, self.config.cortex)
    if self.cortex:
        log.info("[gateway] Cortex memory enabled")
    else:
        log.info("[gateway] Cortex memory disabled")
except Exception as e:
    log.warning("[gateway] Cortex memory unavailable: %s", e)
    self.cortex = None
```

`get_cortex_memory(...)` must return `None` when disabled.

## Message handling integration

At the start of `handle_message()` normal message path after command handling and after `ChatStore.add_message(...)`, create/increment Cortex session best-effort:

```python
cortex_session_id = None
if self.cortex:
    try:
        cortex_session_id = self.cortex.get_or_create_session(channel, user_id, username=username)
        self.cortex.increment_session_count(cortex_session_id, by=1)
    except Exception as e:
        log.warning("[cortex] session tracking failed: %s", e)
        cortex_session_id = None
```

Thread `cortex_session_id` into `_handle_new_message(...)` and `_resume_task(...)` only where practical. MVP can support fast path first; do not break workflow routing.

## Memory retrieval point

In `_handle_new_message()` just before creating `bridge_future`, retrieve memories:

```python
memories = []
if self.cortex and cortex_session_id:
    try:
        memories = self.cortex.search(
            text,
            channel=channel,
            channel_user_id=user_id,
            limit=self.config.cortex.memory_limit,
        )
    except Exception as e:
        log.warning("[cortex] memory search failed: %s", e)
```

Then pass into `_bridge_send_with_file_hints(...)`:

```python
lambda: self._bridge_send_with_file_hints(
    text,
    history,
    channel,
    task_id=cognitive_task_id,
    memories=memories,
)
```

Update helper signature:

```python
def _bridge_send_with_file_hints(
    self,
    text: str,
    history: list,
    channel: str,
    task_id: str = None,
    memories: list | None = None,
) -> tuple[str, list]:
```

Pass to bridge:

```python
response = self.bridge.send(
    text,
    history,
    channel,
    task_id=task_id,
    store=self.task_store,
    memories=memories,
)
```

## Post-response memory write

After bridge succeeds and before returning response from `_handle_new_message()`, write memory candidates best-effort.

Preferred helper:

```python
def _record_cortex_turn(
    self,
    *,
    channel: str,
    user_id: str,
    username: str,
    session_id: str | None,
    user_text: str,
    assistant_text: str,
    source_message_id: int | None = None,
) -> None:
    ...
```

Implementation:

```python
if not self.cortex or not session_id:
    return
try:
    self.cortex.record_turn(
        channel=channel,
        channel_user_id=user_id,
        username=username,
        session_id=session_id,
        user_text=user_text,
        assistant_text=assistant_text,
        source_message_id=source_message_id,
    )
except Exception as e:
    log.warning("[cortex] memory write failed: %s", e)
```

`record_turn()` may live on `CortexMemory` and call extractor/scrubber/create_memory internally.

## Need source message ID

Existing `ChatStore.add_message(...)` returns inserted ID. Capture user message ID in `handle_message()`:

```python
user_message_id = self.store.add_message(...)
```

Thread it into `_handle_new_message()` if feasible. If too invasive for MVP, source may be null, but test should still cover provenance fields that are available.

## `/clear` behavior

Current `/clear` clears active task but not ChatStore history. Do not silently change semantics unless intended.

For Cortex:

```python
if text == "/clear":
    ... existing behavior ...
    if self.cortex:
        try:
            self.cortex.end_session(channel, user_id)
        except Exception as e:
            log.warning("[cortex] clear/end-session failed: %s", e)
    return "Context cleared. Starting fresh."
```

Long-term memories remain. `/clear` means new session epoch, not GDPR delete.

## Workflow routed tasks

MVP can skip memory retrieval/injection for background workflow routed tasks if wiring is risky. Document in code comment:

```python
# Cortex MVP only enriches bridge fast path. Workflow memory injection is a later phase.
```

But normal short chat path must work.

## Logging

Use `log.warning(...)` for failures but never include full user message content in warning logs. Use IDs/counts, not raw content.

Bad:

```python
log.warning("failed to store memory for text=%s", text)
```

Good:

```python
log.warning("[cortex] memory write failed for %s:%s: %s", channel, user_id, e)
```
