---
name: freelance-office
version: 0.2.0
description: |
  Multi-country freelance accounting plugin. One install, many offices
  (DE + AR + ES + US ship in-tree; drop a core/tax/<cc>.py to add more).
  Each office has its own tax regime, locale, currency, invoice
  template, and ledger — zero cross-contamination. Eleven subcommands:
  init, doctor, onboard-client, log-hours, generate-invoice,
  track-expense, pipeline, kleinunternehmer-check, generate-contract,
  dashboard, office (registry CRUD). Every write-side subcommand
  accepts --office <id>; default office is used otherwise.
  Tax regimes handled natively: §19 UStG (DE), Monotributo / Responsable
  Inscripto (AR), IVA 21% / Recargo de Equivalencia / inversión del
  sujeto pasivo (ES), no-federal-VAT pass-through (US). Invoice
  numbers are sidecar-locked per-office-per-year for atomicity.
allowed-tools:
  - freelance-init
  - freelance-doctor
  - freelance-onboard-client
  - freelance-log-hours
  - freelance-generate-invoice
  - freelance-track-expense
  - freelance-pipeline
  - freelance-kleinunternehmer-check
  - freelance-generate-contract
  - freelance-dashboard
category: productivity
tags:
  - freelance
  - accounting
  - multi-country
  - german-tax
  - argentine-tax
  - spanish-tax
  - kleinunternehmer
  - monotributo
  - invoicing
  - pipeline
---

# freelance-office — signed-client accounting for the DE freelance business

`skill-freelance-office` is the Makakoo-native front-end for the
hand-maintained filesystem at `~/freelance-office/`. It turns the
ten manual workflows (onboard a client, log a week, issue an
invoice, track a deductible expense, check the Kleinunternehmer
limit, draft a Projektvereinbarung, read the pipeline, read the
dashboard — plus `init` to bootstrap and `doctor` to sanity-check)
into first-class skill subcommands every Makakoo host can reach.

## Boundary with `skill-career-career-manager`

| State                                    | Owner             | Artifact                                                       |
|------------------------------------------|-------------------|----------------------------------------------------------------|
| Prospecting / outreach / interviewing    | career-manager    | `~/MAKAKOO/data/career-manager/CAREER_LEADS.md` + `leads_data.json` |
| Signed contract / active engagement      | **freelance-office** | `~/freelance-office/clients/<slug>/`                           |

A signed contract is the hand-off. `freelance-office onboard-client`
is the entry point on this side; `--from-lead <ref>` handoff pulling
from career-manager JSON is deferred to a follow-up.

## Subcommands

All subcommands are invoked as `makakoo skill freelance-office <cmd> [args]`
(or from a chat session, via MCP: `skill_discover(query="freelance")`
→ pick the hit → call the right subcommand).

| Subcommand                  | Writes                                                                                             | What it does                                                                 |
|-----------------------------|----------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------|
| `init`                      | `_meta/`, `clients/_template/`, `templates/`, `finances/{YYYY}/`, `admin/`                         | Bootstrap `~/freelance-office/` from bundled templates. Safe to re-run.      |
| `doctor`                    | _(read-only)_                                                                                      | Sanity check: SETTINGS present, RATES parses, counter integrity, YTD counts. |
| `onboard-client`            | `clients/<slug>/meta.yaml`                                                                         | Sign a client: slug, name, sector, rate, terms. `day_rate_agreed` canonical. |
| `log-hours`                 | `clients/<slug>/projects/<p>/_project-tracker.md`                                                   | Upsert KW row, recompute `spent_days` / `remaining_days`, Brain journal.     |
| `generate-invoice`          | `clients/<slug>/projects/<p>/invoices/INV-YYYY-NNN.md`, `finances/{YYYY}/EARNINGS.md`, tracker     | Sidecar-locked atomic number, §19 UStG / Reverse Charge / 19% regime.        |
| `track-expense`             | `finances/{YYYY}/EXPENSES.md`                                                                      | Append row in the right German section (equipment/software/homeoffice/…).    |
| `pipeline`                  | _(read-only)_                                                                                      | Live pipeline table; `--json` envelope for scripting / drift tests.          |
| `kleinunternehmer-check`    | _(read-only)_                                                                                      | YTD vs €22.000 §19 limit, warn at 80%, exit 2 at 100%.                       |
| `generate-contract`         | `clients/<slug>/projects/<p>/contracts/<p>-v{N}.md`                                                | Render Projektvereinbarung. v2/v3 if one already exists — never overwrite.   |
| `dashboard`                 | _(read-only)_                                                                                      | Union of pipeline + this week's hours + next invoice + KU progress + todos.  |

## Capability model

Manifest-declared grants (Layer 2 of the v0.3 three-layer write-
permission model):

```toml
[capabilities]
grants = [
    "fs/read:~/freelance-office",
    "fs/write:~/freelance-office",
    "brain/write",
]
```

No conversational `grant_write_access` prompt is needed during normal
operation — the plugin's write surface is scoped at the manifest
level. A misbehaving subcommand that tries to write outside
`~/freelance-office/**` still fails at the capability server.

## Output modes

- **Default:** human-friendly, colour on TTY.
- `--json`: schema-versioned envelope — same shape as `makakoo perms list --json`.
- `--dry-run`: preview without touching disk (write-side subcommands only).

## State

- Canonical source of truth: `~/freelance-office/` (file tree below).
- Invoice counter: `~/freelance-office/finances/{YYYY}/_invoice_counter.json`, sidecar-locked (`_invoice_counter.json.lock`, `fcntl.flock`, never lock the data fd).
- Plugin-internal state: `$MAKAKOO_HOME/state/skill-freelance-office/` (reserved; nothing yet).

## Deferred to follow-ups (not blocking ship gate)

- `--from-lead <ref>` handoff from career-manager
- PDF render via `markdown_to_pdf`
- lexoffice push integration
- Rate-floor enforcement (`check-rate`)
- Live time-tracker (`start` / `stop`)
- SANCHO autonomous watchdogs (overdue invoices, KU threshold, weekly pipeline sync, homeoffice cap)
- `--mark-paid` + payment reconciliation
- EÜR export
