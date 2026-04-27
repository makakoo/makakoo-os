# Contributing to Makakoo OS

Thanks for showing up. This doc covers how to get a change landed — from "I found a typo" to "I want to ship a new capability verb." Pick the path that fits.

## Ground rules

1. **No CLA.** MIT license. Your contributions stay MIT.
2. **No bots as first contact.** A human writes the first version of every PR description. AI assistance on code is fine (Makakoo eats its own dog food), but the intent and framing are yours.
3. **Atomic commits.** One logical change per commit. Bug fix, test, refactor — separate commits.
4. **Tests required for new logic.** A PR that adds behaviour without adding at least one test gets a "please add a test" reply, not a merge. Regression guards are welcome for every bug you fix.
5. **Cross-platform by default.** Anything new that touches filesystem paths, subprocesses, or environment variables gets cfg-gated (`#[cfg(unix)]` / `#[cfg(windows)]`) before the first push. The CI matrix runs macOS + Linux + Windows on every commit — if the Windows column flips red, fix that before asking for review.
6. **Security: report privately first.** Use `security@makakoo.com` (or open a private GitHub security advisory) for vulnerabilities; don't post them in public issues.

## The smallest change

Typo in a doc, missing `)`, wrong path in a README:

1. Fork.
2. Make the fix on a branch named `fix/<what>`.
3. `git commit -m "docs: fix typo in PHASE_H4_RELOCATE header"`.
4. Open a PR. Title is the commit message. Body is one sentence.
5. Wait for the CI checkmark. Merge happens by a maintainer.

## A code change

```sh
git clone git@github.com:<you>/makakoo-os.git
cd makakoo-os
cargo build --workspace
cargo test --workspace
```

Conventions the codebase leans on:

- **Errors**: `thiserror` for library errors (`InstallError`, `LockError`, `SocketError`), `anyhow` at the CLI boundary. Never `unwrap()` in library code; `.expect("...")` only when you can prove the invariant by construction.
- **Async**: tokio throughout. Short-running CLI subcommands can stay sync + block once at the top with `tokio::runtime::Handle::block_on`.
- **Logging**: `tracing::info!` / `warn!` / `error!`, never `println!` for diagnostic output. The daemon's `sancho tick` heartbeat is the reference — structured key-value pairs, no ad-hoc format strings.
- **Test data**: `tempfile::TempDir` for filesystem tests. Never touch the developer's real `$MAKAKOO_HOME`. Test files under `tests/` are Unix-only unless explicitly gated.
- **Commit messages**: `<phase>-<letter>: <imperative summary>` for sprint work (`phase-d: plugin enable/disable soft toggle`), `<area>: <what>` for maintenance (`docs: add v0.1 release notes`, `ci: cache cargo registry`).

### Before you push

```sh
cargo fmt --all
cargo test --workspace --locked
cargo clippy -p makakoo-platform --all-targets -- -D warnings   # strict lint gate
```

Clippy on the whole workspace still carries pre-Phase-B debt; the strict gate runs only on `makakoo-platform` for now. Don't introduce new warnings; cleanups to existing ones are welcome as their own commits.

### Cross-platform checklist

Anything that:

- Reads or writes a path → use `PathBuf::join` not string concatenation. Assert against `Path::ends_with("segment")` not literal strings.
- Spawns a subprocess → `python3` is Unix-only; Windows wants `python` or `py -3`. Cfg-gate or detect.
- Uses `std::os::unix::*` → wrap in `#[cfg(unix)]` and add a `#[cfg(windows)]` equivalent (or a `SocketError::NotSupported`-style stub).
- Touches env vars → document them in `README.md`'s Environment section + ensure the canonical name is read everywhere (the `MAKAKOO_SOCKET_PATH` / `MAKAKOO_PLUGIN_SOCKET` mismatch that shipped in an earlier Phase E draft cost us three CI rounds).

## Writing a plugin

Plugins are manifest-driven. The manifest is the migration — the runtime code is still `harvey-os/skills/<cat>/<name>/…`. Start with the batch helper:

```sh
python3 scripts/migrate_skill.py <category> <skill_name>
```

That emits `plugins-core/skill-<category>-<name>/plugin.toml` with conservative defaults. Edit it:

- Add `[capabilities].grants` for every verb the skill actually needs (`brain/read`, `llm/chat:minimax/ail-compound`, `net/http:https://api.example.com/*`). Missing grants cause `CapabilityDenied` at runtime — that's the feature.
- Set `[sancho].tasks` if the plugin should tick on a schedule.
- Declare `[depends].plugins` if it relies on a library plugin (like `lib-hte`).
- Pin the blake3 at release time so `distro install` verifies the tarball.

The full schema lives at [`spec/PLUGIN_MANIFEST.md`](spec/PLUGIN_MANIFEST.md).

## Running the dogfood loop locally

Sebastian's install is the reference dogfood target. To mimic it on your box:

```sh
makakoo setup                        # interactive first-run wizard
makakoo distro install core          # materialise the core plugin set
makakoo daemon install               # launchd / systemd / Task Scheduler agent
makakoo infect --global --dry-run    # preview bootstrap writes
makakoo infect --global              # commit them
makakoo sancho status                # confirm tasks are registered
```

`makakoo uninfect` reverses the last step cleanly.

## Reviewing

When you show up to review a PR:

- Read the `## Test plan` section first. If there isn't one, ask for it before reading diff.
- Run the PR locally if the diff touches anything cross-platform. GitHub's CI catches compile errors and unit tests; it doesn't catch "this wizard prompts for the wrong thing."
- Prefer concrete suggestions ("replace `.unwrap()` with `.ok_or(ClientError::BadResponse)?` here") over vague pushes.

## Governance, in one paragraph

No company. No VC. Decisions live in public — issues, PRs, and the `makakoo-os/discussions` board. Maintainers merge; the maintainer list is public at [docs/MAINTAINERS.md](docs/MAINTAINERS.md) when that file exists. A disagreement that a comment thread can't resolve goes to a lazy-consensus 72-hour vote on a GitHub Discussion. If that fails, Sebastian has a BDFL tiebreaker for v0.x; v1.0 opens that up.

## Thanks

Every contribution is read. Every reasonable PR gets a response within a week. Weird PRs — a new mascot sprite, a rewrite of the README in Portuguese, a GPU-accelerated superbrain search — are especially welcome. This project gets more interesting the more directions it grows in.
