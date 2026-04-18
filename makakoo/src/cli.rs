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

    /// Infect CLI global slots with the Makakoo bootstrap block.
    Infect {
        /// Infect global CLI config slots (the only mode wave 5 ships).
        #[arg(long)]
        global: bool,
        /// Preview what would be written without touching any files.
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
pub enum SanchoCmd {
    /// Run every eligible task exactly once.
    Tick,
    /// Show registered tasks and their last-run timestamps.
    Status,
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
