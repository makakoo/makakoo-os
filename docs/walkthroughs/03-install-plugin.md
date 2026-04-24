# Walkthrough 03 — Plugins: see what shipped, toggle one off and back on

## What you'll do

See the list of plugins Makakoo installed with your distro, inspect one in detail, **disable** it (soft turn-off), and **re-enable** it. You'll also learn the three shapes for installing a new plugin from outside the bundled set.

**Time:** about 4 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md) completed and `makakoo install` finished.

## What is a plugin?

A plugin is a self-contained folder with a `plugin.toml` manifest that declares what it ships (a skill, an agent, a SANCHO task, a bootstrap fragment, a library, …). Makakoo lists, inspects, installs, updates, disables, enables, and uninstalls plugins through one subcommand: `makakoo plugin`.

The distro you installed in walkthrough 01 (`core`, by default) came with a curated bundle of plugins already installed. The rest of this walkthrough works with what you already have.

## Steps

### 1. See what's installed

```sh
makakoo plugin list
```

Expected output (wrapping, truncated — yours will be longer):

```text
name                                      version  kind                 language  enabled  source
agent-arbitrage-agent                     1.0.0    Agent                Python    yes      path:/.../plugins-core/agent-arbitrage-agent
agent-browser-harness                     0.1.0    Agent                Python    yes      path:/.../plugins-core/agent-browser-harness
skill-meta-caveman-voice                  0.1.0    BootstrapFragment    Shell     yes      path:/.../plugins-core/skill-meta-caveman-voice
...
```

Every row is one plugin. Columns:
- **name** — globally unique identifier
- **version** — the `version` from its `plugin.toml`
- **kind** — what it provides: `Agent`, `SanchoTask`, `BootstrapFragment`, `Library`, `Cli`, `Skill`
- **enabled** — whether the plugin is currently active
- **source** — where it came from (a local path, a git URL, or a tarball)

> **You will see warning lines** like `WARN skipping plugin — manifest failed to parse` above the table. That's a known issue (DOGFOOD-FINDINGS F-006); the plugin with the broken manifest is silently skipped and everything else works. Ignore the warning.

### 2. Find one that sounds interesting

Let's pick `skill-meta-caveman-voice` — it makes Harvey talk in terse, token-efficient "caveman" voice to save cost. Filter the list:

```sh
makakoo plugin list | grep caveman
```

Expected output:

```text
skill-meta-caveman-voice       0.1.0    BootstrapFragment    Shell     yes      path:/.../plugins-core/skill-meta-caveman-voice
```

### 3. Inspect it in detail

```sh
makakoo plugin info skill-meta-caveman-voice
```

Expected output (truncated after the warning block):

```text
skill-meta-caveman-voice v0.1.0
  summary: Terse, token-efficient internal voice — cuts ~63% of aggregate output tokens
  kind:     BootstrapFragment
  language: Shell
  enabled:  yes
  root:     /Users/you/MAKAKOO/plugins/skill-meta-caveman-voice
  license:  MIT

  effective grants (incl. auto-defaults):
    - infect/contribute

  lock entry:
    installed_at: 2026-04-20T18:52:34+00:00
    blake3:       514f47271a118138bc3aae40b79379c9179de2af1ec9ea09105add9743f72524
    source:       path:/.../plugins-core/skill-meta-caveman-voice
```

Everything a plugin exposes is visible here: the summary, what it can do (`effective grants`), where it lives on disk (`root`), and a cryptographic hash (`blake3`) that proves the code on disk matches what was installed.

### 4. Disable it (soft turn-off)

You don't want to uninstall it, just pause it:

```sh
makakoo plugin disable skill-meta-caveman-voice
```

Expected output:

```text
skill-meta-caveman-voice disabled
restart the daemon (or next sancho tick) to deregister tasks
```

Nothing on disk was deleted. The `plugins.lock` entry flipped `enabled = false`, which means the next registry load will skip this plugin's bootstrap fragment, SANCHO task registration, and MCP tool exposure.

If the plugin did anything ongoing (a SANCHO task on a 5-min interval, say), the daemon either picks up the change on the next tick or you can force it with `makakoo daemon restart`.

### 5. Confirm it's disabled

```sh
makakoo plugin list | grep caveman
```

Expected output — the last column is now `no`:

```text
skill-meta-caveman-voice       0.1.0    BootstrapFragment    Shell     no       path:/.../plugins-core/skill-meta-caveman-voice
```

### 6. Re-enable it

```sh
makakoo plugin enable skill-meta-caveman-voice
```

Expected output:

```text
skill-meta-caveman-voice enabled
```

Back to normal.

## How to install a NEW plugin (the three sources)

Your distro's plugins are already on disk. When you want something outside the bundle, `makakoo plugin install` accepts **three** source shapes:

### Shape 1: A local folder

```sh
makakoo plugin install /path/to/my-plugin
```

Use this when you're developing a plugin locally or have cloned one manually.

### Shape 2: A git URL

```sh
makakoo plugin install git+https://github.com/makakoo/agent-example
makakoo plugin install git+https://github.com/makakoo/agent-example@v1.2.0
makakoo plugin install git+https://github.com/makakoo/agent-example@a1b2c3d4...
```

Pin to a tag or a 40-character SHA. Without a `@<ref>`, Makakoo uses the default branch — OK for trying something out, but pin it for anything you rely on.

### Shape 3: A signed tarball

```sh
makakoo plugin install https://example.com/my-plugin-v1.tar.gz \
  --sha256 <the-expected-hash>
```

Tarballs require an explicit `--sha256`. If the download doesn't match, install aborts before anything lands on disk.

### Shape 4 (bonus): Install from the repo's bundled `plugins-core/`

If you have a checkout of `makakoo-os`, you can install anything from its `plugins-core/` folder by name:

```sh
cd ~/makakoo-os        # or wherever you cloned it
makakoo plugin install --core skill-meta-caveman-voice
```

> **Gotcha (DOGFOOD-FINDINGS F-007):** `--core` resolves `plugins-core/` by walking upward from cwd. It fails if you run it from a directory that isn't inside a checkout. Workaround: `cd` into the checkout first, or set `MAKAKOO_PLUGINS_CORE=/path/to/makakoo-os/plugins-core`.

## Uninstalling

If you've decided you don't want a plugin at all:

```sh
makakoo plugin uninstall <name>
```

Add `--purge` to also wipe the plugin's state directory under `~/MAKAKOO/data/<plugin-name>/`.

## What just happened?

- `makakoo plugin list` reads the `plugins.lock` file and the `~/MAKAKOO/plugins/` tree and joins them into the table you saw. No network calls, no daemon roundtrip.
- `makakoo plugin info <name>` walks the plugin's `plugin.toml`, computes its current blake3, and shows you the lock entry — useful for auditing whether the code on disk matches what was promised when the plugin was installed.
- `disable` and `enable` are **soft** — nothing on disk changes except one boolean in `plugins.lock`. `uninstall` is hard — it removes the plugin directory.
- Plugin installation is content-addressed (blake3 for local/git sources, sha256 for tarballs). That's why `makakoo plugin info` shows the hash: you can verify the code on disk matches a hash you got through a separate channel.

## If something went wrong

| Symptom | Fix |
|---|---|
| `error: plugin not installed: <name>` | That plugin isn't on disk. Check the exact name with `makakoo plugin list` — spelling matters. |
| `error: staging error: target plugin dir already exists — uninstall first` | The plugin is already installed. Either run `makakoo plugin info <name>` to inspect, or uninstall it first if you're trying to reinstall from a different source. |
| `WARN skipping plugin — manifest failed to parse` (spam on every plugin command) | Known bug (DOGFOOD-FINDINGS F-006). The plugin with the broken manifest is harmless but noisy. The rest of your plugins are unaffected. |
| `error: resolve plugins-core: can't find plugins-core/` | You ran `--core` from outside a repo checkout. Either `cd` into your clone of `makakoo-os` or set `MAKAKOO_PLUGINS_CORE=/absolute/path/to/plugins-core`. |
| Plugin still appears active after `disable` | `disable` is read on next registry load. If something was mid-tick, it finishes, then stops. `makakoo daemon restart` forces an immediate reload. |

## Next

- [Walkthrough 04 — Writing to the Brain organically](./04-write-brain-journal.md) — watch the Brain grow without you touching a file.
- [Walkthrough 05 — Ask Harvey](./05-ask-harvey.md) — configure a model provider, then ask a real question.
- [Walkthrough 08 — Use an agent](./08-use-agent.md) — agents are plugins; pick one from the list you just saw and turn it on.
