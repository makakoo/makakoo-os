# 01 — Local Setup and Config

## Goal

Make Cortex Memory opt-in, safe to install, and easy to disable.

## Dependencies

Required MVP dependencies:

```text
none beyond Python stdlib SQLite with FTS5
```

Optional PII dependencies:

```text
presidio-analyzer>=2.2.0
presidio-anonymizer>=2.2.0
spacy>=3.7.0
```

Do not make Presidio mandatory for importing HarveyChat. Import lazily inside scrubber.

If Presidio is missing and `pii_scrubbing=true`, the scrubber must use the built-in fallback regex scrubber. If fallback cannot run, memory write fails closed.

## Config object

Add `CortexConfig` near chat config or in `core/cortex/config.py` and import into `core/chat/config.py`.

```python
@dataclass
class CortexConfig:
    enabled: bool = False
    memory_limit: int = 5
    min_confidence: float = 0.70
    min_importance: float = 0.40
    pii_scrubbing: bool = True
    max_memory_chars: int = 500
    max_prompt_memory_chars: int = 1200
    max_memory_age_days: int = 365
    app_id: str = "makakoo-harveychat"
```

Add to `ChatConfig`:

```python
@dataclass
class ChatConfig:
    telegram: TelegramConfig = field(default_factory=TelegramConfig)
    discord: DiscordConfig = field(default_factory=DiscordConfig)
    bridge: BridgeConfig = field(default_factory=BridgeConfig)
    cortex: CortexConfig = field(default_factory=CortexConfig)
    db_path: str = ""
    log_to_brain: bool = True
    pid_file: str = ""
```

## Config file section

`$MAKAKOO_HOME/data/chat/config.json` may include:

```json
{
  "cortex": {
    "enabled": false,
    "memory_limit": 5,
    "min_confidence": 0.70,
    "min_importance": 0.40,
    "pii_scrubbing": true,
    "max_memory_chars": 500,
    "max_prompt_memory_chars": 1200,
    "max_memory_age_days": 365,
    "app_id": "makakoo-harveychat"
  }
}
```

If config keys are absent, defaults apply.

## Env overrides

Highest priority:

```bash
MAKAKOO_CORTEX_ENABLED=1
MAKAKOO_CORTEX_MEMORY_LIMIT=5
MAKAKOO_CORTEX_MIN_CONFIDENCE=0.70
MAKAKOO_CORTEX_MIN_IMPORTANCE=0.40
MAKAKOO_CORTEX_PII_SCRUBBING=1
MAKAKOO_CORTEX_MAX_MEMORY_CHARS=500
MAKAKOO_CORTEX_MAX_PROMPT_CHARS=1200
MAKAKOO_CORTEX_MAX_AGE_DAYS=365
```

Backward-compatible alias allowed but not required:

```bash
HARVEY_CORTEX_ENABLED=1
```

If both are present, `MAKAKOO_*` wins.

## Disabled mode requirement

When disabled:

- do not instantiate `CortexMemory`
- do not create schema
- do not import Presidio
- do not alter prompt
- do not write memory

This must be covered by an integration test using a temp DB and checking `sqlite_master` has no `cortex_%` tables.

## Manual enablement for dogfood

```bash
cd ~/MAKAKOO/plugins/lib-harvey-core/src
MAKAKOO_CORTEX_ENABLED=1 python3 -m core.chat start --daemon
```

Disable:

```bash
MAKAKOO_CORTEX_ENABLED=0 python3 -m core.chat start --daemon
```

No Docker commands. No Postgres setup. No Redis setup.
