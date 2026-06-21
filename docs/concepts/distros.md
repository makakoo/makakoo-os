# Distro Guide

A Makakoo distro is a TOML bundle of plugins. Use it when you want to
install a known shape of Makakoo instead of picking plugins one by one.

The shipped distro files live in [`distros/`](../../distros/). The current
set is:

| Distro | Best for | Includes |
|---|---|---|
| `minimal` | Bare kernel smoke tests and tiny installs | No extra plugin bundle. |
| `core` | The default install for most users | Brain source picker, browser harness, caveman voice, Headroom, update tasks, switchAILocal, watchdogs, mascot GYM, dreams, and shared storage. |
| `federation` | Multi-machine Brain Network | `core` plus Octopus peer federation and Brain Network skills. |
| `sebastian` | Sebastian's dogfood rig | `core` plus personal agents, skills, and infra defaults. |
| `creator` | Writing, publishing, and media workflows | `core` plus creator-facing plugins. |
| `trader` | Market-facing workflows and future trading agents | `core` plus loop hygiene, canary checks, FTS search, and OpenClaw bridge. Trading-native plugins are still TODO in `distros/trader.toml`. |

Plugin counts change often. Trust `makakoo distro list` on the machine you
are using instead of copying a number from docs.

## Commands

```bash
makakoo distro list
makakoo distro install core
makakoo distro install federation
makakoo distro install --from ./my-distro.toml
makakoo distro save my-current-rig
```

`install` resolves includes, installs every plugin in the effective bundle,
and writes `plugins.lock`. Re-running it is safe: existing plugins are
reused or updated according to the plugin manager.

`save` serializes the currently installed enabled plugin set into a distro
TOML so you can replay it on another machine. Disabled plugins are omitted
by default because the goal is to save the live runtime, not every directory
that happens to be on disk.

## Core distro defaults

`core` is the standard path and the one fresh installs use unless told
otherwise. It includes Headroom by default through `tool-headroom`, so bulky
tool output can be compressed through MCP without an extra setup step. It
also includes `sancho-task-makakoo-update`; the task only performs 24h
updates when `$MAKAKOO_HOME/config/updates.toml` explicitly says
`mode = "auto"`. Fresh setup writes that by default. Existing installs with
no config stay idle.

## Federation distro

`federation` layers Brain Network support on top of `core`. It installs the
peer listener and control-plane skill, but it does not silently expose your
Brain. Activate it explicitly:

```bash
makakoo distro install federation
makakoo network activate --peer-name <node> --bind tailscale
```

## Custom distro shape

A small custom distro looks like this:

```toml
[distro]
name = "my-custom"
display_name = "My Makakoo Rig"
version = "0.1.0"

include = ["core.toml"]

[plugins]
"skill-research-arxiv" = { version = "*" }
"agent-browser-harness" = { version = "*" }
```

Install it with:

```bash
makakoo distro install --from ./my-custom.toml
```
