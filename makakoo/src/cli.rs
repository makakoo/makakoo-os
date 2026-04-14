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
}
