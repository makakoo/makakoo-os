//! clap definitions for the `makakoo` CLI.
//!
//! Every subcommand is a variant on [`Commands`]. T17 appends `Daemon`
//! and `Infect` variants — keep this file touchable as a shared edit
//! boundary across waves 5 and 6.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "makakoo",
    version,
    about = "Makakoo OS — autonomous cognitive extension"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the MCP stdio server (delegates to the `makakoo-mcp` binary).
    Mcp {
        /// Arguments forwarded verbatim to `makakoo-mcp`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Full-text search across the Brain.
    Search {
        /// Query text (use quotes to group).
        query: String,
        /// Maximum hits to return.
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },

    /// Ask a question — FTS retrieval fused with LLM synthesis.
    Query {
        /// Natural-language question.
        question: String,
        /// Number of retrieved hits to stuff into the LLM context.
        #[arg(long, default_value_t = 5)]
        top_k: usize,
        /// Override the LLM model name.
        #[arg(long, default_value = "ail-compound")]
        model: String,
        /// Print the assembled L0+L1+L2 memory block before the LLM
        /// answer (also accessible as `--show-memory`).
        #[arg(short = 'v', long = "show-memory")]
        show_memory: bool,
    },

    /// SANCHO proactive task engine.
    Sancho {
        #[command(subcommand)]
        cmd: SanchoCmd,
    },

    /// Buddy (active mascot) status.
    Buddy {
        #[command(subcommand)]
        cmd: BuddyCmd,
    },

    /// Nursery mascot registry.
    Nursery {
        #[command(subcommand)]
        cmd: NurseryCmd,
    },

    /// Memory consolidation pass ("dream").
    Dream,

    /// Flag a wrong-response funnel entry — manual GYM Layer 1 producer.
    /// Replaces `harvey flag`.
    Flag {
        /// Free-form reason / what was wrong.
        reason: String,
        /// Skill in scope (best-effort hint to Layer 2).
        #[arg(long)]
        skill: Option<String>,
    },

    /// Sync the on-disk Brain (pages/journals/auto-memory) into FTS5.
    /// Replaces Python `superbrain sync`.
    Sync {
        /// Re-index every file regardless of stored content_hash.
        #[arg(long)]
        force: bool,
        /// Also embed any docs that don't have vectors yet (best-effort,
        /// requires a reachable embedding gateway).
        #[arg(long)]
        embed: bool,
        /// Skip the auto-memory dir (default: include if present).
        #[arg(long)]
        no_auto_memory: bool,
        /// Maximum docs to embed in this pass when `--embed` is set.
        #[arg(long, default_value_t = 200)]
        embed_limit: usize,
        /// Index a single file under pages/journals instead of a full
        /// walk. Useful as a post-write hook.
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },

    /// Memory subsystem diagnostics and maintenance.
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },

    /// Print memory promotion candidates.
    Promotions {
        /// Only include candidates scoring at or above this threshold.
        #[arg(long, default_value_t = 0.70)]
        threshold: f32,
        /// Maximum candidates to print.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Run a Python skill by name.
    Skill {
        /// Skill name (e.g. `canary`, `browse`).
        name: String,
        /// Arguments forwarded to the skill's entry script.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Print version, persona, and build metadata.
    Version,

    /// Interactive first-run wizard — name your assistant, pick a
    /// pronoun, pick a default voice. Writes `config/persona.json`.
    /// Refuses to overwrite an existing file unless `--force`.
    Setup {
        /// Overwrite an existing `config/persona.json`.
        #[arg(long)]
        force: bool,
    },

    /// Daemon management — install/uninstall/status/logs/run.
    Daemon {
        #[command(subcommand)]
        cmd: crate::daemon::DaemonCmd,
    },

    /// Infect CLI global slots with the Makakoo bootstrap block AND
    /// the harvey MCP server registration.
    Infect {
        /// Write the bootstrap markdown into every global slot AND
        /// register the harvey MCP server in every CLI's MCP config.
        /// Default mode if no flag given.
        #[arg(long)]
        global: bool,
        /// Write ONLY the MCP server registration (skip bootstrap).
        #[arg(long)]
        mcp: bool,
        /// Audit-only: report drift across all CLIs without writing.
        /// Exit code = 1 if any drift detected (CI-friendly).
        #[arg(long)]
        verify: bool,
        /// Emit drift report as structured JSON on stdout (for watchdogs).
        /// Only meaningful with `--verify`; an error otherwise.
        #[arg(long)]
        json: bool,
        /// Extend `--verify` to also audit per-project (`~/.claude.json`
        /// `projects[*].mcpServers.harvey`), workspace-local `.mcp.json`
        /// files, and prunable `git worktree` records. Only meaningful
        /// with `--verify`. Implies repair when combined with `--repair`.
        #[arg(long)]
        deep: bool,
        /// With `--verify --deep`, apply canonical rewrites to every
        /// zombie entry found. Without `--repair`, `--deep` is read-only.
        #[arg(long)]
        repair: bool,
        /// Preview what would be written without touching any files.
        #[arg(long)]
        dry_run: bool,
        /// Restrict to a comma-separated subset of targets
        /// (claude,gemini,codex,opencode,vibe,qwen,cursor).
        #[arg(long, value_delimiter = ',')]
        target: Vec<String>,
        /// Project-scoped infect: write .harvey/context.md + per-CLI
        /// derivative files (CLAUDE.md, GEMINI.md, AGENTS.md, QWEN.md,
        /// .cursor/rules/makakoo.mdc, .vibe/context.md) in the nearest
        /// project root. Mutually exclusive with --global/--mcp/--verify.
        #[arg(long)]
        local: bool,
        /// Target directory for --local. Default: current directory;
        /// walks up to find .git/ or .harvey/.
        #[arg(long, value_name = "PATH")]
        dir: Option<std::path::PathBuf>,
        /// With --local: only write derivatives for CLIs that have a
        /// ~/.<cli>/ dotdir present. Default is to write all 6 files.
        #[arg(long)]
        detect_installed_only: bool,
        /// With --local: write all 6 derivatives regardless of dotdir
        /// presence (explicit default; useful in CI to document intent).
        #[arg(long)]
        force_all: bool,
        /// With --local: strip harvey:infect-local marker blocks from
        /// derivatives, leaving .harvey/ source files untouched.
        #[arg(long)]
        remove: bool,
        /// With --local: upsert a marker block into the project root
        /// `.gitignore` listing the six derivative paths so they stop
        /// showing as untracked in `git status`. Opt-in.
        #[arg(long)]
        ignore_derivatives: bool,
    },

    /// Uninfect CLI global slots — strip the Makakoo bootstrap block
    /// from every detected AI CLI host's global instructions file.
    ///
    /// Symmetric inverse of `makakoo infect --global`. Removes the
    /// marker-delimited block, deletes the instructions file if it
    /// would be left empty (infect created it → uninfect removes it),
    /// preserves any prose the user wrote around the block.
    Uninfect {
        /// Restrict to a comma-separated subset of targets
        /// (claude,gemini,codex,opencode,vibe,qwen,cursor).
        #[arg(long, value_delimiter = ',')]
        target: Vec<String>,

        /// Preview what would be removed without touching any files.
        #[arg(long)]
        dry_run: bool,
    },

    /// Manage secrets in the OS keyring.
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },

    /// Plugin lifecycle — list, inspect, install, uninstall.
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },

    /// Distro management — list, install a bundle of plugins.
    Distro {
        #[command(subcommand)]
        cmd: DistroCmd,
    },

    /// Prepare `$MAKAKOO_HOME` for kernel use — non-destructive.
    ///
    /// Creates any missing kernel dirs (plugins/, state/, run/, logs/,
    /// config/) and writes a migration marker with a timestamp. Never
    /// touches existing data/, agents/, or harvey-os/. Idempotent.
    Migrate {
        /// Print the plan without creating any dirs.
        #[arg(long)]
        dry_run: bool,
    },

    /// One-shot install: distro + daemon + infect + health check.
    ///
    /// Phase F/1 umbrella command. Runs the existing `distro install`,
    /// `daemon install`, and `infect --global` pipelines in sequence
    /// and prints a unified plan+result summary. Skip individual
    /// steps with `--skip-*` flags, preview the plan with `--dry-run`.
    Install {
        /// Distro to install. Default `core`.
        #[arg(long, default_value = "core")]
        distro: String,

        /// Print what would happen without executing any step.
        #[arg(long)]
        dry_run: bool,

        /// Skip the interactive confirmation on distro install.
        #[arg(long)]
        yes: bool,

        /// Skip the `daemon install` step.
        #[arg(long)]
        skip_daemon: bool,

        /// Skip the `infect --global` step.
        #[arg(long)]
        skip_infect: bool,
    },

    /// Manage JSONL session trees — list, inspect, fork, label, rewind,
    /// export. Gated by `kernel.session_tree = true` in
    /// `$MAKAKOO_HOME/config/kernel.toml` (default OFF).
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },

    /// Manage external AI-agent adapters — list, inspect, install, update,
    /// remove, enable/disable, status, doctor, migrate config, export.
    /// Phase A ships `list`, `info`, and `spec`; later phases add the rest.
    /// Source of truth for the manifest format: `spec/ADAPTER_MANIFEST.md`.
    Adapter {
        #[command(subcommand)]
        cmd: AdapterCmd,
    },

    /// Emit a shell completion script for the chosen shell.
    ///
    /// Write the output to the shell's completion path to enable
    /// tab completion for `makakoo` subcommands, flags, and distro
    /// names. Example installs:
    ///
    ///   zsh:  makakoo completion zsh  > ~/.zfunc/_makakoo
    ///         (ensure `fpath+=~/.zfunc` in .zshrc before compinit)
    ///
    ///   bash: makakoo completion bash > /usr/local/etc/bash_completion.d/makakoo
    ///         (or ~/.local/share/bash-completion/completions/makakoo on Linux)
    ///
    ///   fish: makakoo completion fish > ~/.config/fish/completions/makakoo.fish
    ///
    /// For dynamic completion of installed plugin names, pair with
    /// `makakoo plugin list --json | jq -r '.[].name'` in a shell-
    /// specific completion function (documented in install/completions/).
    Completion {
        /// Target shell. Supported: bash, zsh, fish, elvish, powershell.
        shell: clap_complete::Shell,
    },
}

/// `makakoo plugin <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum PluginCmd {
    /// List every installed plugin with version + hash.
    List {
        /// Emit JSON instead of the default table.
        #[arg(long)]
        json: bool,
    },

    /// Show the parsed manifest + lock entry for one plugin.
    Info {
        /// Plugin name as declared in its `plugin.toml`.
        name: String,
    },

    /// Install a plugin from a local source directory (or a bundled
    /// `plugins-core/<name>` if `--core` is set).
    Install {
        /// Either a local path or — with `--core` — a plugins-core name.
        source: String,

        /// Resolve `source` against `$MAKAKOO_PLUGINS_CORE` (or the
        /// `plugins-core/` dir under the current repo).
        #[arg(long)]
        core: bool,

        /// Expected blake3 of the plugin source tree. Takes precedence
        /// over the value declared in the manifest.
        #[arg(long)]
        blake3: Option<String>,
    },

    /// Remove an installed plugin. With `--purge`, also wipe its state dir.
    Uninstall {
        /// Plugin name.
        name: String,

        /// Wipe the plugin's state dir in addition to its install dir.
        #[arg(long)]
        purge: bool,
    },

    /// Soft-enable a previously-disabled plugin without reinstalling.
    /// The plugin directory stays untouched; the `plugins.lock` entry
    /// flips `enabled = true` and the next registry load picks it up.
    Enable {
        /// Plugin name.
        name: String,
    },

    /// Soft-disable a plugin without uninstalling. SANCHO task
    /// registration + MCP tool exposure + infect fragment emission all
    /// skip the plugin on the next registry load; nothing on disk changes.
    Disable {
        /// Plugin name.
        name: String,
    },

    /// Re-fetch + reinstall the plugin from its recorded source.
    ///
    /// v0.1 only understands `path:` sources (the kind `plugin install`
    /// + `distro install` write). Git URL + tarball sources land with
    /// Phase F. Preserves the plugin's enabled / disabled flag across
    /// the reinstall — if you had disabled it, `update` keeps it disabled.
    /// State directories are preserved (no `--purge`).
    Update {
        /// Plugin name.
        name: String,
    },

    /// Batch-reinstall every plugin from `plugins-core/` into
    /// `$MAKAKOO_HOME/plugins/`. Used after a bulk source migration
    /// (e.g. the self-contained-plugins refactor) when the live install
    /// tree is frozen at the pre-migration shape.
    ///
    /// Walks the plugins-core/ source tree, reinstalls each plugin from
    /// its `[source].path` via `install_from_path`. Preserves existing
    /// enabled/disabled flags. Skips any plugin whose source is missing
    /// or malformed rather than aborting the batch.
    Sync {
        /// Only report what would be reinstalled — do not modify disk.
        #[arg(long)]
        dry_run: bool,

        /// Uninstall + reinstall when a plugin already exists. Without
        /// this flag, sync skips plugins whose target dir is occupied —
        /// safe default. With `--force`, sync calls `uninstall(name)`
        /// then `install_from_path()` atomically per-plugin, preserving
        /// state dirs (no purge). Use when upgrading from the old
        /// manifest-only install shape to self-contained plugins.
        #[arg(long)]
        force: bool,
    },
}

/// `makakoo distro <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum DistroCmd {
    /// List every distro file shipped under `distros/` plus the currently
    /// active distro (if any).
    List,

    /// Install a named distro (`minimal`, `core`, …) or a local file
    /// passed via `--from`. Resolves includes, installs every plugin in
    /// the effective list, writes `plugins.lock`.
    Install {
        /// Distro name as declared inside `distros/<name>.toml`.
        #[arg(required_unless_present = "from")]
        name: Option<String>,

        /// Install from a local distro file instead of the shipped set.
        #[arg(long)]
        from: Option<std::path::PathBuf>,

        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,

        /// Print what would happen without installing anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Serialize the currently-installed plugin set into a distro TOML
    /// file so it can be replayed on another machine. Reads every
    /// enabled entry from `plugins.lock` and pins each to its exact
    /// version + blake3. Disabled plugins are omitted by default so
    /// the distro replays the live runtime, not every dir on disk.
    Save {
        /// Distro name as it will appear inside the saved file and —
        /// by default — as the file stem under `distros/`.
        name: String,

        /// Where to write the distro file. Defaults to
        /// `distros/<name>.toml` if the repo's distros dir can be
        /// located (same resolution as `distro install`).
        #[arg(long)]
        out: Option<std::path::PathBuf>,

        /// Overwrite the target file if it already exists.
        #[arg(long)]
        force: bool,

        /// Include disabled plugins too. Default: emit only
        /// `enabled = true` entries so replays don't resurrect
        /// plugins the user had deliberately turned off.
        #[arg(long)]
        include_disabled: bool,
    },
}

/// `makakoo secret <subcommand>`. Writes go through the OS keyring
/// (Keychain / Secret Service / Credential Manager).
#[derive(Subcommand, Debug)]
pub enum SecretCmd {
    /// Read a secret value from stdin and store it under `key`. The
    /// value is never echoed and never appears in shell history.
    Set {
        /// Canonical key name, e.g. `AIL_API_KEY`.
        key: String,
    },
    /// Retrieve a stored secret and print it to stdout.
    Get {
        /// Canonical key name.
        key: String,
    },
    /// Remove a stored secret.
    Delete {
        /// Canonical key name.
        key: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCmd {
    /// Rewrite legacy `/Users/sebastian/HARVEY/` paths in `recall_log`,
    /// `recall_stats`, and `memory_promotions` to the canonical
    /// `/Users/sebastian/MAKAKOO/` form. Sprint-010 migration.
    PurgeLegacy {
        /// Report counts without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print memory pipeline diagnostics — recall_log, recall_stats,
    /// promoter gate pass-rates, last promoter run.
    Stats {
        /// Emit machine-readable JSON instead of the default table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SanchoCmd {
    /// Run every eligible task exactly once.
    Tick,
    /// Show registered tasks and their last-run timestamps.
    Status,
}

/// `makakoo session <subcommand>`. Every subcommand refuses to run
/// unless `kernel.session_tree = true` — keeps the feature strictly
/// opt-in while G.* stabilizes.
#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// List every session id under `$MAKAKOO_HOME/data/sessions/`.
    List {
        /// Emit JSON (one id per array entry) instead of a plain list.
        #[arg(long)]
        json: bool,
    },

    /// Print a human-readable dump of one session.
    Show {
        /// Session id (matches the JSONL file stem).
        id: String,
        /// Emit the full JSON array instead of the markdown dump.
        #[arg(long)]
        json: bool,
    },

    /// Fork a session at a specific entry into a new session id.
    ///
    /// Non-destructive: source file is untouched, a fresh `<new-id>.jsonl`
    /// is written with a re-rooted header and the kept ancestor chain.
    Fork {
        /// Source session id.
        source: String,
        /// Entry id to fork from (inclusive). Use `session show` to
        /// discover entry ids.
        #[arg(long)]
        from: String,
        /// Override the new session id. Defaults to a time-stamped
        /// derivation of the source id.
        #[arg(long)]
        new_id: Option<String>,
    },

    /// Write a named label entry on the latest entry of a session.
    /// Use `session rewind` to collapse the session back to this point.
    Label {
        /// Session id.
        id: String,
        /// Human-readable label name (unique per session).
        name: String,
    },

    /// Non-destructive rewind to a labeled checkpoint. The pre-rewind
    /// file is preserved as `<id>.<ts>.bak.jsonl` alongside.
    Rewind {
        /// Session id.
        id: String,
        /// Label name previously written via `session label`.
        label: String,
    },

    /// Export a session as Markdown, HTML, or JSON.
    ///
    /// By default prints to stdout — redirect to a file, or pass
    /// `--out <path>` to write atomically.
    Export {
        /// Session id.
        id: String,
        /// Target format.
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Destination file. Default: stdout.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum BuddyCmd {
    /// Print the active buddy's ASCII frame + state line.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum NurseryCmd {
    /// Register a new mascot.
    Hatch {
        /// Unique mascot name.
        name: String,
        /// Species key (from the gimmick LEGO catalog).
        #[arg(long)]
        species: String,
        /// Maintainer handle (e.g. `@schkudlara`).
        #[arg(long)]
        maintainer: String,
        /// One-line job description.
        #[arg(long)]
        job: String,
    },
    /// List every mascot in the registry.
    List,
}

/// `makakoo adapter <subcommand>` — external AI-agent bridge.
#[derive(Subcommand, Debug)]
pub enum AdapterCmd {
    /// List every registered adapter. Reads `~/.makakoo/adapters/registered/`
    /// by default; override via `$MAKAKOO_ADAPTERS_HOME`.
    List {
        /// Emit JSON instead of the default table.
        #[arg(long)]
        json: bool,
        /// Also include bundled reference adapters shipped at
        /// `plugins-core/adapters/<name>/adapter.toml` that are not yet
        /// installed into the user's registry.
        #[arg(long)]
        include_bundled: bool,
    },

    /// Show the parsed manifest + canonical hash for one adapter.
    Info {
        /// Adapter name.
        name: String,
        /// Emit JSON instead of a human-readable dump.
        #[arg(long)]
        json: bool,
    },

    /// Dump the canonical adapter.toml schema description (v1) to stdout.
    /// Reads from the in-binary copy of `spec/ADAPTER_MANIFEST.md`.
    Spec,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_builds_without_panic() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_search_basic() {
        let cli = Cli::try_parse_from(["makakoo", "search", "harvey"]).unwrap();
        match cli.command {
            Commands::Search { query, limit } => {
                assert_eq!(query, "harvey");
                assert_eq!(limit, 10);
            }
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn parse_search_with_limit() {
        let cli =
            Cli::try_parse_from(["makakoo", "search", "--limit", "5", "tytus"]).unwrap();
        if let Commands::Search { query, limit } = cli.command {
            assert_eq!(query, "tytus");
            assert_eq!(limit, 5);
        } else {
            panic!("expected Search");
        }
    }

    #[test]
    fn parse_query_with_top_k() {
        let cli = Cli::try_parse_from([
            "makakoo", "query", "--top-k", "3", "what is lope?",
        ])
        .unwrap();
        if let Commands::Query { question, top_k, .. } = cli.command {
            assert_eq!(question, "what is lope?");
            assert_eq!(top_k, 3);
        } else {
            panic!("expected Query");
        }
    }

    #[test]
    fn parse_sancho_tick() {
        let cli = Cli::try_parse_from(["makakoo", "sancho", "tick"]).unwrap();
        matches!(cli.command, Commands::Sancho { cmd: SanchoCmd::Tick });
    }

    #[test]
    fn parse_sancho_status() {
        let cli = Cli::try_parse_from(["makakoo", "sancho", "status"]).unwrap();
        if let Commands::Sancho { cmd } = cli.command {
            matches!(cmd, SanchoCmd::Status);
        } else {
            panic!("expected Sancho");
        }
    }

    #[test]
    fn parse_buddy_status() {
        let cli = Cli::try_parse_from(["makakoo", "buddy", "status"]).unwrap();
        if let Commands::Buddy { cmd } = cli.command {
            matches!(cmd, BuddyCmd::Status);
        } else {
            panic!("expected Buddy");
        }
    }

    #[test]
    fn parse_nursery_hatch_full() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "nursery",
            "hatch",
            "Olibia",
            "--species",
            "owl",
            "--maintainer",
            "@schkudlara",
            "--job",
            "test patrol",
        ])
        .unwrap();
        if let Commands::Nursery {
            cmd:
                NurseryCmd::Hatch {
                    name,
                    species,
                    maintainer,
                    job,
                },
        } = cli.command
        {
            assert_eq!(name, "Olibia");
            assert_eq!(species, "owl");
            assert_eq!(maintainer, "@schkudlara");
            assert_eq!(job, "test patrol");
        } else {
            panic!("expected Nursery::Hatch");
        }
    }

    #[test]
    fn parse_nursery_list() {
        let cli = Cli::try_parse_from(["makakoo", "nursery", "list"]).unwrap();
        if let Commands::Nursery { cmd } = cli.command {
            matches!(cmd, NurseryCmd::List);
        } else {
            panic!("expected Nursery");
        }
    }

    #[test]
    fn parse_skill_with_args() {
        let cli = Cli::try_parse_from([
            "makakoo", "skill", "canary", "run", "opencode", "--workspace", "clean",
        ])
        .unwrap();
        if let Commands::Skill { name, args } = cli.command {
            assert_eq!(name, "canary");
            assert_eq!(args, vec!["run", "opencode", "--workspace", "clean"]);
        } else {
            panic!("expected Skill");
        }
    }

    #[test]
    fn parse_dream() {
        let cli = Cli::try_parse_from(["makakoo", "dream"]).unwrap();
        matches!(cli.command, Commands::Dream);
    }

    #[test]
    fn parse_promotions_defaults() {
        let cli = Cli::try_parse_from(["makakoo", "promotions"]).unwrap();
        if let Commands::Promotions { threshold, limit } = cli.command {
            assert!((threshold - 0.70).abs() < 1e-6);
            assert_eq!(limit, 10);
        } else {
            panic!("expected Promotions");
        }
    }

    #[test]
    fn parse_version() {
        let cli = Cli::try_parse_from(["makakoo", "version"]).unwrap();
        matches!(cli.command, Commands::Version);
    }

    #[test]
    fn parse_mcp_with_passthrough_args() {
        let cli = Cli::try_parse_from(["makakoo", "mcp", "--list-tools"]).unwrap();
        if let Commands::Mcp { args } = cli.command {
            assert_eq!(args, vec!["--list-tools"]);
        } else {
            panic!("expected Mcp");
        }
    }

    #[test]
    fn parse_plugin_list() {
        let cli = Cli::try_parse_from(["makakoo", "plugin", "list"]).unwrap();
        match cli.command {
            Commands::Plugin {
                cmd: PluginCmd::List { json: false },
            } => {}
            _ => panic!("expected Plugin::List"),
        }
    }

    #[test]
    fn parse_plugin_list_json() {
        let cli = Cli::try_parse_from(["makakoo", "plugin", "list", "--json"]).unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::List { json },
        } = cli.command
        {
            assert!(json);
        } else {
            panic!("expected Plugin::List");
        }
    }

    #[test]
    fn parse_plugin_info() {
        let cli = Cli::try_parse_from(["makakoo", "plugin", "info", "mascot-gym"]).unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::Info { name },
        } = cli.command
        {
            assert_eq!(name, "mascot-gym");
        } else {
            panic!("expected Plugin::Info");
        }
    }

    #[test]
    fn parse_plugin_install_core() {
        let cli = Cli::try_parse_from([
            "makakoo", "plugin", "install", "--core", "mascot-gym",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd:
                PluginCmd::Install {
                    source,
                    core,
                    blake3,
                },
        } = cli.command
        {
            assert_eq!(source, "mascot-gym");
            assert!(core);
            assert!(blake3.is_none());
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_install_local_path() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "install",
            "/tmp/my-plugin",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::Install { source, core, .. },
        } = cli.command
        {
            assert_eq!(source, "/tmp/my-plugin");
            assert!(!core);
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_uninstall_with_purge() {
        let cli = Cli::try_parse_from([
            "makakoo", "plugin", "uninstall", "mascot-gym", "--purge",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::Uninstall { name, purge },
        } = cli.command
        {
            assert_eq!(name, "mascot-gym");
            assert!(purge);
        } else {
            panic!("expected Plugin::Uninstall");
        }
    }

    #[test]
    fn parse_distro_list() {
        let cli = Cli::try_parse_from(["makakoo", "distro", "list"]).unwrap();
        matches!(
            cli.command,
            Commands::Distro {
                cmd: DistroCmd::List
            }
        );
    }

    #[test]
    fn parse_distro_install_named() {
        let cli = Cli::try_parse_from([
            "makakoo", "distro", "install", "core", "--yes",
        ])
        .unwrap();
        if let Commands::Distro {
            cmd:
                DistroCmd::Install {
                    name,
                    from,
                    yes,
                    dry_run,
                },
        } = cli.command
        {
            assert_eq!(name.as_deref(), Some("core"));
            assert!(from.is_none());
            assert!(yes);
            assert!(!dry_run);
        } else {
            panic!("expected Distro::Install");
        }
    }

    #[test]
    fn parse_install_defaults() {
        let cli = Cli::try_parse_from(["makakoo", "install"]).unwrap();
        if let Commands::Install {
            distro,
            dry_run,
            yes,
            skip_daemon,
            skip_infect,
        } = cli.command
        {
            assert_eq!(distro, "core");
            assert!(!dry_run);
            assert!(!yes);
            assert!(!skip_daemon);
            assert!(!skip_infect);
        } else {
            panic!("expected Install");
        }
    }

    #[test]
    fn parse_install_with_flags() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "install",
            "--distro",
            "minimal",
            "--dry-run",
            "--skip-daemon",
        ])
        .unwrap();
        if let Commands::Install {
            distro,
            dry_run,
            skip_daemon,
            skip_infect,
            ..
        } = cli.command
        {
            assert_eq!(distro, "minimal");
            assert!(dry_run);
            assert!(skip_daemon);
            assert!(!skip_infect);
        } else {
            panic!("expected Install");
        }
    }

    #[test]
    fn parse_session_list_default() {
        let cli = Cli::try_parse_from(["makakoo", "session", "list"]).unwrap();
        match cli.command {
            Commands::Session {
                cmd: SessionCmd::List { json: false },
            } => {}
            _ => panic!("expected Session::List"),
        }
    }

    #[test]
    fn parse_session_fork_with_new_id() {
        let cli = Cli::try_parse_from([
            "makakoo", "session", "fork", "abc",
            "--from", "m3", "--new-id", "abc-alt",
        ])
        .unwrap();
        if let Commands::Session {
            cmd: SessionCmd::Fork { source, from, new_id },
        } = cli.command
        {
            assert_eq!(source, "abc");
            assert_eq!(from, "m3");
            assert_eq!(new_id.as_deref(), Some("abc-alt"));
        } else {
            panic!("expected Session::Fork");
        }
    }

    #[test]
    fn parse_session_label() {
        let cli =
            Cli::try_parse_from(["makakoo", "session", "label", "abc", "before-tool"]).unwrap();
        if let Commands::Session {
            cmd: SessionCmd::Label { id, name },
        } = cli.command
        {
            assert_eq!(id, "abc");
            assert_eq!(name, "before-tool");
        } else {
            panic!("expected Session::Label");
        }
    }

    #[test]
    fn parse_session_rewind() {
        let cli =
            Cli::try_parse_from(["makakoo", "session", "rewind", "abc", "before-tool"]).unwrap();
        if let Commands::Session {
            cmd: SessionCmd::Rewind { id, label },
        } = cli.command
        {
            assert_eq!(id, "abc");
            assert_eq!(label, "before-tool");
        } else {
            panic!("expected Session::Rewind");
        }
    }

    #[test]
    fn parse_session_export_html_to_file() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "session",
            "export",
            "abc",
            "--format",
            "html",
            "--out",
            "/tmp/out.html",
        ])
        .unwrap();
        if let Commands::Session {
            cmd: SessionCmd::Export { id, format, out },
        } = cli.command
        {
            assert_eq!(id, "abc");
            assert_eq!(format, "html");
            assert_eq!(out.unwrap().to_str(), Some("/tmp/out.html"));
        } else {
            panic!("expected Session::Export");
        }
    }

    #[test]
    fn parse_distro_install_from_file() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "distro",
            "install",
            "--from",
            "/tmp/custom.toml",
            "--dry-run",
        ])
        .unwrap();
        if let Commands::Distro {
            cmd:
                DistroCmd::Install {
                    name,
                    from,
                    yes: _,
                    dry_run,
                },
        } = cli.command
        {
            assert!(name.is_none());
            assert_eq!(from.as_deref().map(|p| p.to_str().unwrap()), Some("/tmp/custom.toml"));
            assert!(dry_run);
        } else {
            panic!("expected Distro::Install");
        }
    }
}
