# `agent-pg-watchdog`

**Summary:** Periodic PostgreSQL health checks and schema drift detection.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-pg-watchdog/`

## When to use

When you have a Postgres database Makakoo cares about (a downstream app, a RAG corpus store, a shared workspace DB) and want Harvey to:

- Run a periodic health probe (connection reachable, replication lag, slow-query count).
- Detect schema drift against a committed reference snapshot.
- Leave `[[pg-watchdog]]` journal breadcrumbs on anomalies.

Disable if you don't use Postgres.

## Prerequisites

- A reachable Postgres instance.
- `DATABASE_URL` env var (or per-database entries in `~/MAKAKOO/config/pg-watchdog/dbs.toml`).

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-pg-watchdog
makakoo plugin disable agent-pg-watchdog
makakoo plugin enable agent-pg-watchdog
makakoo daemon restart
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-pg-watchdog/` — last health snapshot per DB, reference schemas.
- **Journal breadcrumbs:** `- [[pg-watchdog]] <db>: <finding>` in today's Brain journal when anomalies are detected.
- **Logs:** `~/MAKAKOO/data/logs/agent-pg-watchdog.{out,err}.log`

## Health signals

- `ps -ef | grep pg_watchdog` — one running process.
- `~/MAKAKOO/state/agent-pg-watchdog/last_probe.json` updated in the last poll interval.
- Recent `[[pg-watchdog]]` journal lines (or none, which is healthy — no anomalies).

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `psycopg2.OperationalError: could not connect` | DB down, network partition, or credentials rotated | Verify `$DATABASE_URL`; check DB is up. |
| Schema-drift alerts every run | Reference schema out of date | `pg_dump --schema-only > ~/MAKAKOO/state/agent-pg-watchdog/<db>-reference.sql` to update the baseline. |
| No journal breadcrumbs at all on a busy DB | Probe interval too long for your use | Lower the interval in `~/MAKAKOO/config/pg-watchdog/dbs.toml`. |

## Capability surface

- `net/tcp:*` — Postgres TCP connections.
- `fs/read` + `fs/write` — own state dir + journal writes.
- `secret/read:database.*` — DB credentials.

## Remove permanently

```sh
makakoo plugin uninstall agent-pg-watchdog --purge
```
