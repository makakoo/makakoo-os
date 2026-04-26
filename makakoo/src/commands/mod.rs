//! Subcommand dispatch.
//!
//! The `dispatch` function consumes a parsed [`Commands`] variant and
//! runs the matching command module. Each handler returns an exit code
//! (0 on success, nonzero on any "expected" failure like a missing
//! subsystem). Unexpected failures propagate as `anyhow::Error` and
//! `main.rs` prints them via `output::print_error`.

pub mod adapter;
pub mod adapter_gen;
pub mod agent;
pub mod agent_slot;
pub mod bucket;
pub mod buddy;
pub mod distro;
pub mod dream;
pub mod flag;
pub mod install;
pub mod lifecycle;
pub mod mcp;
pub mod memory;
pub mod migrate;
pub mod nursery;
pub mod octopus;
pub mod perms;
pub mod plugin;
pub mod promotions;
pub mod query;
pub mod s3;
pub mod s3_endpoint;
pub mod sancho;
pub mod search;
pub mod session;
pub mod setup;
pub mod sync;
pub mod skill;
pub mod version;

use crate::cli::Commands;
use crate::context::CliContext;

/// Dispatch a parsed [`Commands`] to its command module. Returns the
/// exit code `main` should forward to the OS.
pub async fn dispatch(cmd: Commands, ctx: &CliContext) -> anyhow::Result<i32> {
    match cmd {
        Commands::Mcp { args } => mcp::run(args),
        Commands::Search { query, limit } => search::run(ctx, &query, limit).await,
        Commands::Query {
            question,
            top_k,
            model,
            show_memory,
        } => query::run(ctx, &question, top_k, &model, show_memory).await,
        Commands::Sancho { cmd } => sancho::run(ctx, cmd).await,
        Commands::Buddy { cmd } => buddy::run(ctx, cmd),
        Commands::Nursery { cmd } => nursery::run(ctx, cmd),
        Commands::Dream => dream::run(ctx).await,
        Commands::Flag { reason, skill } => flag::run(ctx, &reason, skill),
        Commands::Sync {
            force,
            embed,
            no_auto_memory,
            embed_limit,
            file,
        } => sync::run(ctx, force, embed, no_auto_memory, embed_limit, file).await,
        Commands::Memory { cmd } => memory::run(ctx, cmd).await,
        Commands::Promotions { threshold, limit } => {
            promotions::run(ctx, threshold, limit)
        }
        Commands::Skill { name, args } => skill::run(&name, &args, ctx).await,
        Commands::Version => version::run(),
        Commands::Setup {
            section,
            only,
            skip,
            non_interactive,
            reset,
            force,
        } => setup::run(setup::SetupArgs {
            section,
            only,
            skip,
            non_interactive,
            reset,
            force,
        }),
        Commands::Daemon { cmd } => {
            crate::daemon::dispatch(cmd).await?;
            Ok(0)
        }
        Commands::Infect {
            global,
            mcp,
            verify,
            json,
            deep,
            repair,
            dry_run,
            target,
            local,
            dir,
            detect_installed_only,
            force_all,
            remove,
            ignore_derivatives,
        } => {
            crate::infect::dispatch(crate::infect::InfectArgs {
                global,
                mcp,
                verify,
                json,
                deep,
                repair,
                dry_run,
                target,
                local,
                dir,
                detect_installed_only,
                force_all,
                remove,
                ignore_derivatives,
            })
            .await
        }
        Commands::Secret { cmd } => dispatch_secret(cmd),
        Commands::Plugin { cmd } => plugin::run(ctx, cmd).await,
        Commands::Distro { cmd } => distro::run(ctx, cmd).await,
        cmd @ Commands::Install { .. } => install::dispatch(ctx, cmd).await,
        Commands::Migrate { dry_run } => migrate::run(ctx, dry_run).await,
        Commands::Completion { shell } => dispatch_completion(shell),
        Commands::Uninfect { target, dry_run } => {
            crate::infect::uninfect_global(target, dry_run).await
        }
        Commands::Perms { cmd } => perms::run(ctx, cmd).await,
        Commands::Session { cmd } => session::run(ctx, cmd).await,
        Commands::Adapter { cmd } => adapter::run(ctx, cmd).await,
        Commands::Octopus { args } => octopus::run(args),
        Commands::Agent { cmd } => agent::run(ctx, cmd),
        Commands::S3 { cmd } => s3::run(ctx, cmd),
        Commands::Bucket { cmd } => bucket::run(ctx, cmd).await,
    }
}

fn dispatch_completion(shell: clap_complete::Shell) -> anyhow::Result<i32> {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    let bin_name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(0)
}

fn dispatch_secret(cmd: crate::cli::SecretCmd) -> anyhow::Result<i32> {
    use crate::cli::SecretCmd;
    use crate::secrets::SecretsStore;
    match cmd {
        SecretCmd::Set { key } => {
            // Read value from stdin so it never touches shell history.
            use std::io::{BufRead, Write};
            let stdin = std::io::stdin();
            let mut stderr = std::io::stderr();
            let _ = write!(stderr, "value for {key}: ");
            let _ = stderr.flush();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;
            let value = line.trim_end_matches(['\n', '\r']).to_string();
            if value.is_empty() {
                anyhow::bail!("refusing to store empty value");
            }
            SecretsStore::set(&key, &value)?;
            println!("stored {key} in keyring");
            Ok(0)
        }
        SecretCmd::Get { key } => {
            let v = SecretsStore::get(&key)?;
            println!("{v}");
            Ok(0)
        }
        SecretCmd::Delete { key } => {
            SecretsStore::delete(&key)?;
            println!("deleted {key} from keyring");
            Ok(0)
        }
    }
}
