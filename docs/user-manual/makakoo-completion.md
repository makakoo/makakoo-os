# `makakoo completion` — CLI reference

`makakoo completion` emits a shell completion script for the shell you name.
Piping the output to the right location enables tab-completion for every
`makakoo` subcommand, flag, and (with a small wrapper) installed plugin
names. It is a one-time setup step — do it once after install and you get
completions forever across upgrades, since the binary always emits the
current surface.

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

## Key use patterns

### Install completions (one-time, per shell)

```sh
# zsh — write to a fpath dir and re-source
mkdir -p ~/.zfunc
makakoo completion zsh > ~/.zfunc/_makakoo
# add to ~/.zshrc (before compinit):  fpath+=(~/.zfunc)
# then reload:
exec zsh

# bash — system-wide location (macOS with homebrew bash-completion)
makakoo completion bash > /usr/local/etc/bash_completion.d/makakoo
# or per-user on Linux:
makakoo completion bash > ~/.local/share/bash-completion/completions/makakoo

# fish
makakoo completion fish > ~/.config/fish/completions/makakoo.fish
```

### Verify completions are active

```sh
# type makakoo <TAB> — you should see the subcommand list
makakoo <TAB>
makakoo plugin <TAB>
```

### Dynamic completion of installed plugin names

The static completion script covers flags and fixed subcommands. For
dynamic completion of plugin names, add a shell-specific wrapper that
calls `makakoo plugin list --json` at completion time. See
`docs/install/completions/` in the source tree for ready-to-use snippets
for each shell.

## Related commands

- [`makakoo-plugin.md`](makakoo-plugin.md) — plugin names auto-completed via the dynamic wrapper
- [`setup-wizard.md`](setup-wizard.md) — the setup wizard links to completion setup after install
- [`index.md`](index.md) — user manual index

## Common gotcha

**`makakoo <TAB>` shows nothing in zsh after following the steps above.**
The most likely cause is that `fpath+=(~/.zfunc)` appears *after* `compinit`
in your `~/.zshrc`. The `fpath` extension must come before `compinit`. Move
it above the `compinit` call, then run `exec zsh` to reload.
If completions still do not appear, confirm with
`echo $fpath | tr ' ' '\n' | grep zfunc` that the dir is in the path
and that `~/.zfunc/_makakoo` is non-empty.
