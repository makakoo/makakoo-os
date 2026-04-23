# `makakoo sancho`

Manage the SANCHO proactive task engine. SANCHO ticks every 30 minutes (default) and runs maintenance tasks registered by plugins.

## Example

```sh
makakoo sancho status          # registered tasks + last-run state
makakoo sancho tick            # force one tick now
makakoo sancho list --json
```

## Full command surface

Run with `--help` for every flag, subcommand, and exit code:

```sh
makakoo sancho --help
```

See also: [User manual index](index.md), [Use cases](../use-cases.md), [Troubleshooting](../troubleshooting/index.md).
