# makakoo shell completion

`makakoo completion <shell>` emits a completion script for bash, zsh, fish, elvish, or powershell. The generated script covers every subcommand + flag — `makakoo plugin <TAB>` completes to `list / info / install / uninstall / enable / disable / update`, `makakoo distro install <TAB>` suggests distro names, etc.

## Install

### zsh (macOS default + Linux)

```sh
mkdir -p ~/.zfunc
makakoo completion zsh > ~/.zfunc/_makakoo
```

Then ensure `~/.zfunc` is on `fpath` **before** `compinit` runs. Add to `~/.zshrc`:

```zsh
fpath+=("$HOME/.zfunc")
autoload -Uz compinit && compinit
```

### bash (Linux default, bash-completion installed)

```sh
# User-level (no sudo):
mkdir -p ~/.local/share/bash-completion/completions
makakoo completion bash > ~/.local/share/bash-completion/completions/makakoo

# System-wide (macOS Homebrew):
makakoo completion bash | sudo tee "$(brew --prefix)/etc/bash_completion.d/makakoo" >/dev/null
```

Open a new shell.

### fish

```sh
makakoo completion fish > ~/.config/fish/completions/makakoo.fish
```

## Dynamic plugin-name completion

The static scripts complete subcommands and flags but don't know the set of plugins currently installed. For live `<TAB>` completion on `makakoo plugin info <TAB>` against the installed set, add a thin wrapper in your shell init:

### zsh

```zsh
_makakoo_plugin_names() {
    local names
    names=(${(f)"$(makakoo plugin list --json 2>/dev/null | jq -r '.[].name')"})
    compadd -a names
}
# Chain onto the generated completion:
compdef _makakoo_plugin_names makakoo
```

### bash

```bash
__makakoo_plugin_names() {
    makakoo plugin list --json 2>/dev/null | jq -r '.[].name'
}
complete -F _makakoo -o bashdefault -o default \
    -C '__makakoo_plugin_names' makakoo
```

(Requires `jq`. Works for `plugin {info, uninstall, enable, disable, update}` positional slots.)

## Verify

```sh
makakoo <TAB><TAB>      # should list all top-level subcommands
makakoo plugin <TAB>    # list/info/install/uninstall/enable/disable/update
makakoo distro <TAB>    # list/install/save
```
