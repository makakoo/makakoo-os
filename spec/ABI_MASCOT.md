# ABI: Mascot — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `mascot`
**Owner:** Makakoo kernel, `crates/core/src/nursery/`
**Promotes to v1.0:** after Phase E dogfooding

---

## 0. What a mascot is

A **mascot** is a named character with stats and a patrol function.
Mascots are Harvey's minor helpers — Olibia (the seal, the default
Makakoo mascot), Pixel (debugging specialist), Cinder (syntax checker),
Ziggy (docstring patrol), Glimmer (TODO watcher).

Mascots run periodic patrol functions via the SANCHO scheduler and
report findings to the Brain. They are essentially named,
personality-bearing SANCHO tasks with a fixed "species" archetype.

## 1. Contract

A mascot plugin is a directory containing:

- `plugin.toml` with `kind = "mascot"`
- A `[mascot]` table with species + stats + patrol function
- A `[sancho.tasks]` entry for the patrol
- Entrypoint script with the patrol function
- Optional `species.json` with art/voice/flavor data

## 2. Minimal manifest

```toml
[plugin]
name = "mascot-olibia"
version = "2.1.0"
kind = "mascot"
language = "python"
summary = "The official Makakoo mascot — Olibia the seal"

[source]
path = "plugins-core/mascot-olibia"

[abi]
mascot = "^0.1"
sancho-task = "^0.1"

[depends]
python = ">=3.11"
plugins = ["brain ^1.0"]

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
run = ".venv/bin/python -m olibia.patrol"

[capabilities]
grants = ["brain/read", "brain/write", "state/plugin"]

[mascot]
species = "seal"
stats = { friendliness = 95, patience = 80, snark = 20 }
patrol = "olibia.patrol:tick"

[sancho]
tasks = [
  { name = "olibia_patrol", interval = "3600s" },
]

[state]
dir = "$MAKAKOO_HOME/state/mascot-olibia"
```

## 3. The `[mascot]` table

| Field | Required | Type | Meaning |
|---|---|---|---|
| `species` | yes | string | Archetype tag (e.g. `seal`, `sloth`, `squirrel`, `butterfly`) |
| `stats` | yes | table | Integer stats from the species' known stat set |
| `patrol` | yes | string | Handler reference in the plugin's language |
| `flavor` | no | string | Short one-line personality description |
| `art` | no | path | Optional ASCII art file for CLI display |
| `voice` | no | path | Optional voice/catchphrase file |

### 3.1 Species archetypes

v0.1 recognized species:

| Species | Personality | Typical stats |
|---|---|---|
| `seal` | Friendly, playful, welcoming | friendliness, patience, snark |
| `sloth` | Slow, thorough, methodical | debugging, patience, snark |
| `squirrel` | Fast, frantic, collection-focused | speed, attention, hoarding |
| `butterfly` | Observant, aesthetic, fleeting | observation, beauty, fragility |
| `owl` | Wise, nocturnal, analytical | wisdom, nocturnality, silence |
| `fox` | Clever, sneaky, opportunistic | cunning, stealth, adaptability |

New species can be added in v0.2+ by shipping a species manifest file
in the kernel repo. v0.1 hardcodes the species list above.

### 3.2 Stats

Stats are integers 0-100. Each species has a canonical stat list (see
above table). Mascots can omit stats they don't care about; unspecified
stats default to 50.

Stats are used by:
- **Nursery UI** — `makakoo nursery list` shows each mascot's stat block
- **Patrol scheduling** — mascots with higher `attention` stat patrol
  more frequently
- **Brain journal coloring** — mascot findings are tagged with their
  name + stat summary for fun

## 4. Patrol function

The patrol function is called periodically by SANCHO. Contract:

```python
def tick() -> dict:
    """
    Run one patrol cycle. Return a dict with:
      - findings: list of things noticed (one-line strings)
      - urgency: "low" | "medium" | "high"
      - journal_entry: optional markdown block to append to today's journal
    """
    return {
        "findings": ["found 3 orphan Brain pages", "one TODO older than 30 days"],
        "urgency": "low",
        "journal_entry": "- [[Glimmer]] found 1 stale TODO and 3 orphans today",
    }
```

The kernel:
1. Spawns the patrol via the same subprocess contract as SANCHO tasks
2. Captures the JSON output
3. If `journal_entry` present, appends to `$MAKAKOO_HOME/data/Brain/journals/<today>.md`
4. Logs findings to `$MAKAKOO_HOME/logs/mascots/<name>/patrol.jsonl`
5. If `urgency = "high"`, writes a flag file at `$MAKAKOO_HOME/data/mascot-alerts/<name>.flag`

## 5. Nursery and buddy

The **nursery** is the registry of all installed mascots. The **buddy**
is the currently active mascot whose ASCII art and voice appear in CLI
output (configurable via `makakoo buddy set <name>`).

Every mascot plugin automatically registers in the nursery at install
time. `makakoo nursery list` walks the list. `makakoo buddy set
<name>` marks one as active.

## 6. Forbidden for mascots at v0.1

- **Long-running processes.** Mascots patrol periodically via SANCHO,
  not continuously. If you need a continuous process, make it an
  agent, not a mascot.
- **Cross-plugin stats poking.** A mascot's stats live in its own
  manifest and state dir. No "rising stats from usage" at v0.1.
- **Multiple mascots per plugin.** One plugin = one mascot. Plugins
  that want to ship a whole team ship multiple plugins.

## 7. Versioning

Same semver rules.

## 8. Example: `mascot-olibia`

See manifest above. Olibia is the default buddy (the little seal
greeting in `makakoo buddy status`). She patrols once per hour via
`olibia_patrol`, checks the Brain for new journal entries, and writes
friendly welcome lines. Stats: friendliness 95, patience 80, snark 20.

Her patrol function is intentionally cheerful — she's the face of
Makakoo for new users.

---

**Status:** v0.1 LOCKED.
