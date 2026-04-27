# Phase H.4 — Runtime relocation runbook

Moves Sebastian's live install from `~/MAKAKOO/` to `~/.makakoo/` and
drops a compat symlink so every legacy reference (scripts, config
hardcodes, plugin manifests) keeps working. This is the physical
manifestation of D10 (clean source/runtime split).

**This runbook is manual by design.** A fully automated `makakoo
relocate` subcommand is a future enhancement — the risk profile
(stop daemon, move 3 GB of Brain + vectors + chat logs, rewrite
config, restart) wants a human finger on the keyboard the first
time. The steps below are small, reversible up to step 4, and have
a dry-run checklist at each stage.

## Why

- **Spec D10**: `/Users/sebastian/makakoo-os/` is source (git checkout).
  `~/.makakoo/` is runtime (data + plugins + state + logs).
  `~/MAKAKOO/` today conflates both.
- **Multi-user future**: a second account on the same machine wants
  its own `~/.makakoo/` that doesn't step on Sebastian's. Dotfile-
  prefixed home is the only honest answer.
- **Cleaner uninstall**: `rm -rf ~/.makakoo && rm ~/MAKAKOO` restores
  the user's home to pre-Makakoo state in one step.

## Pre-flight

1. **Disk check** — `~/.makakoo/` gets whatever `~/MAKAKOO/` currently
   weighs. Verify `du -sh ~/MAKAKOO` fits in your target filesystem.
2. **Daemon stop** — if `launchctl list com.makakoo.daemon` shows a
   running PID, stop it: `launchctl bootout gui/$(id -u)/com.makakoo.daemon`
   (or `makakoo daemon uninstall` for the full reset).
3. **Backup** — `cp -R ~/MAKAKOO ~/MAKAKOO.backup-$(date +%Y%m%d)`.
   Only delete the backup after step 5 verification passes.
4. **Snapshot env** — `echo $MAKAKOO_HOME $HARVEY_HOME` + grep your
   shell rcs for `MAKAKOO_HOME` or `HARVEY_HOME` exports. You'll
   rewrite these.

## Move

```sh
# 1. Physical move. NO symlink yet — clean rename.
mv ~/MAKAKOO ~/.makakoo

# 2. Compat symlink. Every legacy reference to ~/MAKAKOO now
#    resolves through the symlink to ~/.makakoo.
ln -s ~/.makakoo ~/MAKAKOO

# 3. Rewrite env vars to canonical.
#    ~/.zshrc:
export MAKAKOO_HOME="$HOME/.makakoo"
export HARVEY_HOME="$HOME/.makakoo"   # legacy alias, same dir

# 4. Rewrite any absolute paths in Sebastian's install that baked
#    the legacy literal. Mostly config files + plugin.lock entries.
grep -rl '/Users/sebastian/MAKAKOO' ~/.makakoo/config ~/.makakoo/plugins | \
    xargs sed -i '' 's|/Users/sebastian/MAKAKOO|/Users/sebastian/.makakoo|g'

# 5. Reinstall the daemon under the new root.
makakoo daemon install
launchctl list com.makakoo.daemon | grep PID   # expect a real PID
```

## Verify

```sh
tail -f ~/.makakoo/data/logs/makakoo.err.log
# Expect `sancho tick: N/M tasks ok` within 5 min (or run
# `makakoo sancho tick` once to force).

makakoo brain search "<a phrase you know is indexed>"
# Expect hits — proves superbrain + Brain filesystem are intact.

makakoo plugin list
# Expect the same 18 plugins you had before the move.

ls -la ~/MAKAKOO
# Expect `lrwxr-xr-x ... ~/MAKAKOO -> /Users/<you>/.makakoo`.
```

## Rollback

Up through step 4 the old layout still exists as `~/MAKAKOO.backup-<date>/`.

```sh
# Tear down.
makakoo daemon uninstall
unlink ~/MAKAKOO
mv ~/.makakoo ~/.makakoo.failed-$(date +%s)
mv ~/MAKAKOO.backup-<date> ~/MAKAKOO

# Restore env.
unset MAKAKOO_HOME
# (or restore it to the pre-change value if you had one)

makakoo daemon install
```

## What this does NOT touch

- `~/MAKAKOO/agents/*` submodules — the move takes them along; no
  git-level changes. Submodule retirement is a separate Phase H.4
  task (`git subtree merge` for each).
- `/Users/sebastian/makakoo-os/` source checkout — untouched.
  Source lives next to `~/.makakoo/` runtime, as D10 mandates.
- `~/HARVEY` legacy symlink — delete it separately if you still have
  it. Check with `ls -la ~/HARVEY` before rm.

## When to run

- Not before v0.1 is tagged — relocating mid-launch is bad for
  morale. Post-launch stabilization window is the right moment.
- After a full daemon/brain backup.
- With a clean `git status` on the makakoo-os source checkout so
  any fallback commits are reversible.
- NOT during a deadline week — this is a 20-minute op if nothing
  explodes, half a day if something does.
