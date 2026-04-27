# HarveyChat Agentic Flow — Multi-Turn Task Refinement

## Problem

Previously, HarveyChat treated each Telegram message as an isolated Q&A:
- Message arrives → LLM response → send response → done
- No persistent task state across messages
- No way for Harvey to ask clarifying questions and wait for user feedback
- Long-running tasks would block responses

This resulted in the "New Thread" UI symptom: every message felt like starting fresh.

## Solution: Persistent Task Queue + Conversation State

HarveyChat now tracks persistent tasks with state machines:

```
QUEUED → RUNNING → {AWAITING_INPUT} → COMPLETED
                   ↑            ↓
                   └────────────┘
                    (user provides feedback)
```

### Architecture

**Files:**
- `task_queue.py` — SQLite-backed task storage with state tracking
- `conversation.py` — Per-user conversation state and message classification
- `gateway.py` — Updated to use tasks + classification

### How It Works

#### 1. User sends a message
```python
# Gateway receives: "generate an image of a cute owl"
conv = self.conv_manager.get_or_create(channel, user_id)
active_task = conv.get_active_task()

if active_task and active_task.awaiting_input_prompt:
    # User is responding to "What style?"
    response = await self._resume_task(active_task, message, ...)
else:
    # New conversation
    response = await self._handle_new_message(message, ...)
```

#### 2. Harvey can ask for clarification
Bridge system prompt already says:
```
If you need more info, ask the user.
Use the [[SEND_PHOTO:...]] marker to send images you generate.
```

When Harvey's response includes a question, the system marks task as AWAITING_INPUT:
```python
task.state = TaskState.AWAITING_INPUT
task.awaiting_input_prompt = "What style would you like?"
task_queue.update_task(task)
```

#### 3. User provides feedback in next message
Gateway detects user is responding to pending question:
```python
message_type = conv.classify_message(user_text)
# Returns: "continue_task" (user is responding) or "start_new" (new work)

if message_type == "continue_task":
    response = await self._resume_task(active_task, user_text, ...)
```

#### 4. Harvey refines and sends result
```
Initial: "Generating image... needs clarification on style"
User: "cute owl in watercolor"
Refined: "Here's your owl!" + [[SEND_PHOTO:~/pics/owl.png]]
```

## Task Lifecycle Example

**User in Telegram:** "generate an image for my profile"

```
[GATEWAY]
  Creates task: goal="generate image for my profile"
  Task state: QUEUED → RUNNING

[AGENT/BRIDGE]
  Calls LLM with: "User wants image for profile. Need details on style/size."
  LLM response: "What style — photorealistic, cartoon, abstract?"

[TASK QUEUE]
  Updates task:
    state: AWAITING_INPUT
    awaiting_input_prompt: "What style?"

[TELEGRAM]
  Sends to user: "What style — photorealistic, cartoon, abstract?"
  UI shows: Message delivered, waiting for response

---

**User responds:** "cute cartoon owl"

[GATEWAY]
  Detects: active_task.awaiting_input_prompt exists
  → Resume existing task, don't start new

[AGENT/BRIDGE]
  History includes all prior messages + original request
  Calls LLM with: [original msg] + [clarification request] + [user response]
  LLM generates image, marks to send

[TELEGRAM]
  Sends: "Here's your owl!" + [image file]
  Task: COMPLETED

---

**User can refine:** "make it more photorealistic"

[GATEWAY]
  Detects: task already completed
  → Start NEW task for refinement

[AGENT]
  New task with context from previous work
  Iterates again
```

## Progress Updates (Future Enhancement)

For long-running tasks, Harvey can send updates:

```python
task.set_running(task)

# During work:
task.add_progress("Generating base image...")
await send_message(channel, user_id, "Working... generating base image")

task.add_progress("Applying style...")
await send_message(channel, user_id, "Refining... applying style")

task.set_completed(task, result, files=[...])
await send_message(channel, user_id, result)
```

## Configuration

**Max history messages** (config.json):
```json
{
  "bridge": {
    "max_history_messages": 20
  }
}
```

This is the window of conversation context passed to each LLM call. Full task history lives in task.messages.

## Benefits

✅ **True agentic flow** — Harvey can do complex multi-step work
✅ **Clarification loops** — Ask user questions, wait for answers
✅ **Progress updates** — Send "working..." messages during long tasks
✅ **Context persistence** — Full conversation history across messages
✅ **Task resumption** — Users can refine results iteratively
✅ **No "New Thread"** — Messages feel like continuous conversation

## Implementation Checklist

- [x] TaskQueue system with SQLite persistence
- [x] Task state machine (QUEUED → RUNNING → AWAITING_INPUT → COMPLETED)
- [x] ConversationState for per-user tracking
- [x] Message classification (new vs. continue)
- [x] Gateway integration with task flow
- [x] File sending via task.files_to_send
- [ ] **NEXT:** Update bridge.py to detect clarification requests and auto-set AWAITING_INPUT
- [ ] **NEXT:** Implement progress message sending during long tasks
- [ ] **NEXT:** Add Telegram inline buttons for quick feedback (✅ Yes, ❌ No, 📝 Refine)
- [ ] **NEXT:** Background executor for async tasks (don't block on long jobs)

## Testing

```bash
# Start HarveyChat
python3 -m core.chat start

# In Telegram, try:
# 1. "generate image of cute owl"
# 2. When asked for style: "watercolor cartoon"
# 3. When received: "make it more playful"

# Check task history:
sqlite3 ~/MAKAKOO/data/chat/tasks.db "SELECT id, goal, state FROM tasks;"
```

## See Also

- `bridge.py` — LLM interface with file marker support
- `gateway.py` — Main router with task integration
- `telegram.py` — Channel handler (no changes needed)
- `store.py` — Message history (complementary to task.messages)
