# `makakoo infect`

Write the Makakoo bootstrap block into every detected CLI host's global config file (~/.claude/CLAUDE.md, ~/.gemini/GEMINI.md, etc.). Re-runnable: uses content-hash diff so it won't touch files that already have the latest block. Never edits your shell dotfiles.

## Example

```sh
makakoo infect --verify        # check drift
makakoo infect                 # write / update
makakoo infect --target claude,gemini
```

## Full command surface

Run with `--help` for every flag, subcommand, and exit code:

```sh
makakoo infect --help
```

See also: [User manual index](index.md), [Use cases](../use-cases.md), [Troubleshooting](../troubleshooting/index.md).
