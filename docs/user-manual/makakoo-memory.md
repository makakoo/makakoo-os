# `makakoo memory` — diagnostics and maintenance

`makakoo memory` is for checking the memory pipeline itself. It does not write
journal entries. For Brain search, use `makakoo search` or `makakoo query`.

## Commands

| Command | Purpose |
|---|---|
| `makakoo memory stats` | Print recall log counts, promoter gate pass-rates, and last promoter run. |
| `makakoo memory purge-legacy` | Rewrite old `/Users/sebastian/HARVEY/` paths in memory tables to `/Users/sebastian/MAKAKOO/`. Mostly a migration repair tool. |

## Examples

```bash
makakoo memory stats
makakoo memory purge-legacy
```
