# skill-freelance-office

First-class accounting plugin for a freelancer's filesystem. One install,
**many offices**: you can run a DE office, an AR office, and an ES office
from the same Makakoo machine — each with its own tax regime, locale,
currency, invoice template, and ledger. Makes the ten-ish manual workflows
every freelancer actually does (onboard a client, log a week of hours,
issue an invoice, track a deductible expense, watch the local tax
threshold, draft a Projektvereinbarung / Projektvereinbarung / contrato,
read the pipeline, read the dashboard) reachable from **every** Makakoo
host — Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen, pi,
HarveyChat.

**Status:** v0.3.0 · shipped 2026-04-21 · **144 tests green** · DE + AR + ES + US regimes · invoice lifecycle complete (PDF · mark-paid · SANCHO watchdogs).

## Multi-country at a glance

| Country | Module            | Currency | Default regime             | Key behavior                                                  |
|---------|-------------------|----------|----------------------------|---------------------------------------------------------------|
| DE      | `core/tax/de.py`  | EUR      | +19% USt                   | Kleinunternehmer (§19 UStG, €22k limit) + Reverse Charge EU  |
| AR      | `core/tax/ar.py`  | ARS      | Monotributo (no IVA)       | Category A–K thresholds + Responsable Inscripto (21% IVA)    |
| ES      | `core/tax/es.py`  | EUR      | IVA 21%                    | Inversión del sujeto pasivo (EU B2B) + Recargo de Equivalencia |
| US      | `core/tax/us.py`  | USD      | No federal VAT             | Pass-through (state sales tax = client responsibility)        |

Add a country by dropping a `core/tax/<cc>.py` file matching the `TaxRegime`
protocol. No plugin edits required.

## Register an office

```bash
# First office — defaults to it
makakoo skill freelance-office office add --id de-main --path ~/freelance-office-de --country DE --default
# Second office — different country + currency
makakoo skill freelance-office office add --id ar-main --path ~/freelance-office-ar --country AR
makakoo skill freelance-office office list
# Switch default
makakoo skill freelance-office office use ar-main
```

Every write-side subcommand accepts `--office <id>`:

```bash
makakoo skill freelance-office --office de-main init
makakoo skill freelance-office --office ar-main init
# Auto-picks up: es-AR locale, ARS currency, Spanish INVOICE template,
# Monotributo threshold table, AR-specific EXPENSES categories.

makakoo skill freelance-office --office ar-main \
    onboard-client --slug buenosaires-co --name "BA Co" --day-rate 2000000
# meta.yaml ← currency: ARS  (inherited from office, NOT hardcoded EUR)

makakoo skill freelance-office --office ar-main \
    generate-invoice --client buenosaires-co --project p1 --days 10 \
    --description "Sprint 1" --issued 2026-04-01
# → INV-2026-001 in ARS, Monotributo "sin IVA discriminado" block
```

**Counters are per-office-per-year.** DE office's INV-2026-001 and AR
office's INV-2026-001 are two independent files, independent numeric
sequences — no collision, no shared state.

## v0.1 → v0.2 zero-action migration

v0.1 installs are auto-upgraded on the first v0.2 command:

- Registry `$MAKAKOO_HOME/config/freelance_offices.json` is seeded with
  one entry pointing at the existing `~/freelance-office/`, id `default`,
  country `DE`.
- SETTINGS.yaml gains a top-level `office:` block. Every other byte of
  the file is preserved.

Subsequent runs are no-ops (idempotent, lock-protected). Existing v0.1
workflows keep working — no edits to your muscle memory.

## Install

The plugin lives at `plugins-core/skill-freelance-office` in the Makakoo repo.
On a fresh machine:

```bash
# One-time: install the Python client as a library plugin (dep)
MAKAKOO_PLUGINS_CORE=~/makakoo-os/plugins-core \
    makakoo plugin install --core lib-makakoo-client

# Install the skill itself
MAKAKOO_PLUGINS_CORE=~/makakoo-os/plugins-core \
    makakoo plugin install --core skill-freelance-office

# Bootstrap ~/freelance-office/ from bundled templates
makakoo skill freelance-office init
```

Verify:

```bash
makakoo plugin list | grep freelance-office
makakoo skill freelance-office -- --help    # lists all 10 subcommands
makakoo skill freelance-office doctor       # sanity check — should be green
```

## Boundary with `skill-career-career-manager`

| State                                       | Owner             | Where                                                         |
|---------------------------------------------|-------------------|---------------------------------------------------------------|
| Prospecting / outreach / interviewing       | career-manager    | `~/MAKAKOO/data/career-manager/CAREER_LEADS.md`, `leads_data.json` |
| Signed contract / active engagement         | **freelance-office** | `~/freelance-office/clients/<slug>/`                          |

`freelance-office onboard-client` is the hand-off. A future
`--from-lead <ref>` flag pulling from career-manager JSON is deferred.

## The ten subcommands

### `init`
Bootstrap `~/freelance-office/` from bundled templates. Idempotent — existing
files are never overwritten.

```bash
makakoo skill freelance-office init
makakoo skill freelance-office init --dry-run   # preview
```

### `doctor`
Read-only sanity check. Exits 0 when green, 1 when any check fails.

```bash
makakoo skill freelance-office doctor
makakoo skill freelance-office doctor --json
```

### `onboard-client`
Sign a new client.

```bash
makakoo skill freelance-office onboard-client \
    --slug northbound --name "Northbound GmbH" --sector "SaaS" \
    --contact-email ops@northbound.com --day-rate 1400 \
    --payment-terms-days 30
```

Canonical schema field: `day_rate_agreed` — every other subcommand reads it.

### `log-hours`
Upsert a calendar-week row in `_project-tracker.md`. Additive across calls.

```bash
makakoo skill freelance-office log-hours \
    --client northbound --project platform-migration --week 17 \
    --hours '{"Mo":8,"Di":8,"Mi":8,"Do":8}' --note "sprint 1"
```

### `generate-invoice`
Atomic `INV-YYYY-NNN` allocation, render, book to EARNINGS + tracker.

Rate resolution precedence:

1. `--amount-net <N>` → use as-is.
2. `--days <D>` → `amount_net = D × client.day_rate_agreed` (error if the
   client's `day_rate_agreed` is missing).
3. Neither passed → error.

VAT regime auto-detected:

- SETTINGS `kleinunternehmer: true` → §19 UStG block, no VAT.
- `client.b2b && client.client_country != "DE" && client.ust_id` → Reverse Charge.
- Else → +19% USt.

```bash
makakoo skill freelance-office generate-invoice \
    --client northbound --project platform-migration \
    --days 20 --description "Sprint 1 — 20 days" \
    --leistungszeitraum "2026-04-01 bis 2026-04-30"
```

### `track-expense`
Append to the right section of `EXPENSES.md`. Section is chosen by `--category`
(one of `equipment | software | fortbildung | homeoffice | telefon | fahrt | arbeitsmittel`).

```bash
makakoo skill freelance-office track-expense \
    --date 2026-04-20 --amount-net 149 --ust 28.31 \
    --category software --description "lexoffice annual subscription"
```

### `pipeline`
Read-only table of every (client × project): invoiced / paid / outstanding /
overdue. `--json` dumps a schema-versioned envelope.

```bash
makakoo skill freelance-office pipeline --json
```

### `kleinunternehmer-check`
§19 UStG YTD vs €22.000. 80% → stderr warning. 100% → exit 2.

```bash
makakoo skill freelance-office kleinunternehmer-check
```

### `generate-contract`
Render a Projektvereinbarung. First call = v1; every subsequent call bumps
to v2, v3…

```bash
makakoo skill freelance-office generate-contract \
    --client northbound --project platform-migration \
    --title "Platform Migration" \
    --description "Migrate legacy stack to Rust." \
    --total-days 40
```

### `dashboard`
Union of pipeline + this week's hours + next invoice + KU progress +
top-3 TODOs. `--json` for scripting.

```bash
makakoo skill freelance-office dashboard
```

## How to use from a chat session

Every MCP-capable CLI can drive this plugin without any new infrastructure.
The agent's flow:

1. User says *"log 8 hours Monday on northbound platform migration"*.
2. Agent calls `skill_discover(query="freelance")` → finds this SKILL.md.
3. Agent reads the SKILL.md to learn the subcommand shape.
4. Agent calls `makakoo skill freelance-office log-hours --client northbound --project platform-migration --week <KW> --hours '{"Mo":8}' --note "..."`.
5. Plugin writes the tracker, writes the Brain journal, returns a JSON envelope.
6. Agent quotes the tool's `message` back to the user.

No conversational `grant_write_access` prompt is needed — the plugin's write
surface is declared in its `plugin.toml` at install time.

## Capability model

Layer-2 (manifest) grants only:

```toml
[capabilities]
grants = [
    "fs/read:~/freelance-office",
    "fs/write:~/freelance-office",
    "brain/write",
]
```

The `brain/write` grant is used to append one outliner line to today's
`~/MAKAKOO/data/Brain/journals/YYYY_MM_DD.md` per successful write-side
subcommand, tagged with `[[freelance-office]]`. No direct filesystem write to
the Brain tree — everything routes through the kernel's capability socket via
`makakoo_client.Client.brain_write_journal`.

## Tests

```bash
cd ~/MAKAKOO/plugins/skill-freelance-office && python3 -m pytest tests/
```

144 tests (v0.3):

- Per-subcommand (init, doctor, onboard, log-hours, generate-invoice,
  mark-paid, track-expense, pipeline, kleinunternehmer-check,
  generate-contract, dashboard) — happy + error + dry-run paths.
- Invoice counter — 10-process race, disk-seed, corrupt counter, peek.
- Brain integration — five write-side commands each emit one line with the
  expected `[[wikilink]]` shape; dry-run never touches the Brain.
- ISO-8601 week boundary — locks the convention used by `dashboard`.
- Format fidelity — appending / mark-paid on
  `finances/2026/{EARNINGS,EXPENSES}.md` never touches prose, section headers,
  or non-target tables.
- md-table parser hardening — pipe-inside-a-cell is routed to the sentinel,
  never silently shifts cell indices.
- PDF render — valid PDF bytes + content-matches-invoice-data + paid-guard
  + --force regeneration.
- mark-paid — full / partial / two-tranche accumulation / over-payment /
  unknown-invoice / idempotent / tracker two-phase verify.
- SANCHO overdue — grace window / re-fire guard / Telegram floor / state
  round-trip / read-then-confirm race guard / orphan purge.
- SANCHO threshold — first 80% / no-refire / 100% crossing / per-office
  isolation / office-rename isolation.
- SANCHO daily digest — fires on activity / silent on zero-activity /
  multi-office composition.
- End-to-end smoke + cross-office PDF smoke.

## v0.3 new features (invoice lifecycle)

### `generate-invoice --pdf`

Renders the Markdown invoice to a PDF next to the `.md` via weasyprint.
No LaTeX, no kernel-capability grant, in-process.

```bash
makakoo skill freelance-office generate-invoice \
    --client acme --project p1 --amount-net 5000 \
    --description "May consulting" --pdf
# writes clients/acme/projects/p1/invoices/INV-2026-001.md + .pdf
```

**Paid-guard:** regenerating the PDF of an already-paid invoice errors
with `Invoice [[INV-...]] already paid. Use --force to regenerate.`
Pass `--force` if you really mean it — the PDF is re-rendered, the
EARNINGS and tracker rows are left alone (no double-booking).

### `mark-paid`

```bash
makakoo skill freelance-office mark-paid \
    --invoice INV-2026-007 --paid-date 2026-05-19
# → ✅ bezahlt, Zahlungseingänge appended, tracker [✅], Brain journal line.

# Partial payments accumulate:
makakoo skill freelance-office mark-paid --invoice INV-2026-007 --amount 4000
# → 💰 teilweise, balance €7,200.00
makakoo skill freelance-office mark-paid --invoice INV-2026-007 --amount 7200
# → ✅ bezahlt (auto-flip on cumulative ≥ net)
```

The tracker write is verified by re-reading the file after the mutation.
A dropped tracker write is surfaced loudly instead of leaving EARNINGS
and the tracker out of sync.

### SANCHO watchdogs

Three handlers declared in `plugin.toml [[sancho]]`:

- `freelance_invoice_overdue_tick` — runs every 24h. Fires one Brain
  journal line per overdue invoice per day and one Telegram ping when
  the net crosses `SETTINGS.finance.overdue_ping_floor` (default €500).
  Read-then-confirm: if a concurrent `mark-paid` flips the row between
  the scan and the send, the Telegram ping is cancelled.
- `freelance_threshold_tick` — runs every 24h. Fires once on the first
  80% crossing and once on the first 100% crossing per office per
  calendar year.
- `freelance_daily_digest_tick` — runs at 09:00 local. Sends ONE
  summary if any state changed in the last 24h; silent otherwise.

## Deferred

- `onboard-client --from-lead <ref>` handoff from career-manager.
- `freelance-office reconcile` — backfill EARNINGS rows for invoices
  produced outside the plugin (referenced by `mark-paid`'s error hint).
- `current_status` auto-transition (`prospecting → active` on first
  contract) — waits on a proper client-lifecycle spec.
- lexoffice / FacturaScripts / AFIP push.
- Rate-floor enforcement (currently a warning).
- Live time-tracker (`start` / `stop`).
- Email-the-PDF automation.
- EÜR / modelo-130 / DDJJ exports.
