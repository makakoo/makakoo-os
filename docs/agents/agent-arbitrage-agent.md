# `agent-arbitrage-agent`

**Summary:** Monitors Polymarket CLOB for negative-risk arbitrage opportunities.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-arbitrage-agent/`

## When to use

When you actively trade on Polymarket and want Harvey to surface **negative-risk arbitrage** — situations where buying a set of complementary outcomes locks in a guaranteed profit regardless of event resolution.

**Not on by default in every distro** — if you don't trade, disable it to silence its SANCHO heartbeat:

```sh
makakoo plugin disable agent-arbitrage-agent
makakoo daemon restart
```

## Prerequisites

- (Optional) Polymarket account credentials for placing trades. For read-only monitoring, no credentials needed.
- `~/MAKAKOO/config/arbitrage-agent/config.toml` — watchlist of markets + thresholds.

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-arbitrage-agent
makakoo plugin disable agent-arbitrage-agent
makakoo plugin enable agent-arbitrage-agent
makakoo daemon restart
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-arbitrage-agent/` — last-seen prices, open positions, fitness history.
- **Data:** `~/MAKAKOO/data/arbitrage-agent/v2/` — signal log, paper trades, evolution log.
- **Logs:** `~/MAKAKOO/data/logs/agent-arbitrage-agent.{out,err}.log`

## Health signals

- `ps -ef | grep arbitrage-agent` — one running process.
- `tail -20 ~/MAKAKOO/data/arbitrage-agent/v2/signals_log.jsonl` — recent market probes (one JSONL row per tick).
- `makakoo plugin info agent-arbitrage-agent` — `enabled: yes`.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Agent runs but signal log is empty | Polymarket API rate limit or network issue | Check `err.log`; if many `429`s, lower poll frequency in config. |
| `ModuleNotFoundError: py_clob_client` | Python deps not installed | Reinstall the plugin: `makakoo plugin install --core agent-arbitrage-agent`. |
| Paper-trade P&L diverges wildly from expected | Stale watchlist (markets closed / resolved) | Edit `~/MAKAKOO/config/arbitrage-agent/config.toml` and prune resolved market IDs. |

## Capability surface

- `net/http:clob-api.polymarket.com,gamma-api.polymarket.com` — market data + trading.
- `fs/read` + `fs/write` — own state + data dirs.
- `secret/read` — trading credentials (only if configured).

## Remove permanently

```sh
makakoo plugin uninstall agent-arbitrage-agent --purge
```

`--purge` deletes the signal log history. Omit if you want to preserve it.
