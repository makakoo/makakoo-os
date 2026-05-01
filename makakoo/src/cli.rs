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
    /// Subcommand. Optional so bare `makakoo` (no args) can land on a
    /// friendly first-run banner instead of clap's "subcommand required"
    /// error. The same Tytus v0.6 Phase A pattern.
    #[command(subcommand)]
    pub command: Option<Commands>,
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

    /// Run a pattern — one-shot LLM dispatch with strategy + mascot
    /// composition. Pattern primitive shipped by
    /// SPRINT-PATTERN-SUBSTRATE-V1. Pattern name resolves with or
    /// without the `pattern-` prefix (e.g. `summarize` or
    /// `pattern-summarize`). Input arrives via `--input`, an
    /// `--input @file` path, or stdin when `--input -` is set.
    Run {
        /// Pattern name. The `pattern-` directory prefix is optional.
        pattern: String,
        /// User input as a literal string, `@/path/to/file` to read from
        /// disk, or `-` to read from stdin. Bound to the canonical
        /// `input` variable. Omit to skip.
        #[arg(short = 'i', long)]
        input: Option<String>,
        /// Set additional variables — `--var name=value`. Repeatable.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Override the mascot persona (e.g. `olibia`).
        #[arg(long)]
        mascot: Option<String>,
        /// Override the strategy (`cot`, `tot`, `react`,
        /// `harvey-rigor`, `caveman`).
        #[arg(long)]
        strategy: Option<String>,
        /// Override the model. Resolution: flag > pattern.toml >
        /// FABRIC_MODEL_<NAME> env > kernel default.
        #[arg(long)]
        model: Option<String>,
        /// Override the vendor. Same precedence sans env.
        #[arg(long)]
        vendor: Option<String>,
        /// Compose without firing — prints route + composed messages
        /// to stdout. No network call. Exits 0.
        #[arg(long)]
        dry_run: bool,
        /// Validate the response is JSON and emit it raw. Non-JSON
        /// responses produce exit code 2 with the body on stderr.
        #[arg(long)]
        json: bool,
    },

    /// Print version, persona, and build metadata.
    Version,

    /// Interactive setup wizard — a re-runnable dispatcher with one
    /// section per configurable area (persona, brain, cli-agent,
    /// terminal, model-provider, infect). Run with no args to walk
    /// every section in order; pass a section name to run just one;
    /// or use `--only` / `--skip` to scope.
    Setup {
        /// Run only this section. If omitted, every section runs in
        /// order. Valid names: `persona`, `brain`, `cli-agent`,
        /// `terminal` (macOS only), `model-provider`, `infect`.
        section: Option<String>,
        /// Run only the given sections (comma-separated or repeat).
        /// Wins over `--skip` when both are set.
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Skip these sections (comma-separated or repeat).
        #[arg(long, value_delimiter = ',')]
        skip: Vec<String>,
        /// Don't prompt — print current state and exit 0. Also the
        /// default behavior when stdin isn't a TTY.
        #[arg(long)]
        non_interactive: bool,
        /// Wipe `$MAKAKOO_HOME/state/makakoo-setup/completed.json`
        /// before running so every section re-asks.
        #[arg(long)]
        reset: bool,
        /// Re-run the persona section and overwrite an existing
        /// `config/persona.json`. Other sections ignore this flag.
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

        /// Skip the post-install `setup` wizard hand-off. By default a
        /// successful install offers to run `makakoo setup` interactively
        /// — use this flag in CI / unattended installs. Non-TTY installs
        /// never prompt regardless.
        #[arg(long)]
        no_setup: bool,
    },

    /// Manage user-managed write permissions — the runtime Layer-3 of
    /// the three-layer capability model (spec/CAPABILITIES.md §1.11).
    ///
    /// Grants extend Harvey's baseline sandbox without re-compiling or
    /// restarting the daemon. Default duration is 1 hour — pass
    /// `--for permanent` only for stable, reviewed access.
    ///
    ///   makakoo perms list                                   # show active grants
    ///   makakoo perms grant ~/work/                          # 1h default
    ///   makakoo perms grant ~/work/ --for 24h --label today  # time-limited
    ///   makakoo perms revoke g_20260421_abcd1234             # by id
    ///   makakoo perms purge                                  # drop expired
    ///   makakoo perms audit --since 1h                       # recent activity
    Perms {
        #[command(subcommand)]
        cmd: PermsCmd,
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

    /// Harvey Octopus — signed-MCP peer federation.
    ///
    /// Peer a Tytus pod, another Mac, or an SME teammate with this
    /// host so they can read/write your Brain via signed MCP. Thin
    /// passthrough to the Python `core.octopus.bootstrap_wizard`
    /// shipped with `lib-harvey-core`.
    ///
    /// Subcommands:
    ///   makakoo octopus bootstrap [--peer-name N] [--force]
    ///   makakoo octopus invite [--link] [--peer-name N] [--scope S] [--duration D]
    ///   makakoo octopus join <token-or-link> [--peer-name N] [--pubkey KEY]
    ///   makakoo octopus trust list [--all] [--json]
    ///   makakoo octopus trust revoke <peer-name> [--reason R]
    ///   makakoo octopus doctor
    Octopus {
        /// Arguments forwarded verbatim to the Python wizard.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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

    /// Drive an agent plugin's lifecycle entrypoint.
    ///
    /// Agent plugins declare `[entrypoint].start|stop|health` in their
    /// `plugin.toml`. This subcommand resolves a plugin by name,
    /// reads the relevant entry, runs it with `cwd = plugin.root` via
    /// `/bin/sh -c`, and forwards the exit code. Thin wrapper — the
    /// daemon is the primary lifecycle supervisor; this command is the
    /// escape hatch for manual control, SKILL.md examples, and the
    /// `sancho-task-plugin-update-check/post_update` hook.
    ///
    /// Subcommands:
    ///   makakoo agent start  <name>
    ///   makakoo agent stop   <name>
    ///   makakoo agent status <name>
    ///   makakoo agent health <name>
    ///
    /// `status` is not declared in plugin manifests today; it is derived
    /// by invoking `[entrypoint].health` if present, else falling back to
    /// a pgrep-style scan on the plugin name.
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },

    /// S3 endpoint operations — bootstrap the Makakoo-owned service
    /// keypair against the local Garage instance.
    ///
    /// Subcommands:
    ///   makakoo s3 bootstrap [--force-rotate]
    S3 {
        #[command(subcommand)]
        cmd: S3Cmd,
    },

    /// Bucket lifecycle — create / grant / revoke / expire on top of
    /// the local Garage backend (v0.7.1; AWS/R2/B2/Minio land in v0.8).
    ///
    /// Subcommands:
    ///   makakoo bucket create   <name>  [--ttl <dur>] [--quota <size>]
    ///   makakoo bucket list                                   [--json]
    ///   makakoo bucket info     <name>                        [--json]
    ///   makakoo bucket grant    <name> --to <label> --perms <r,w>
    ///                                                         [--ttl <dur>]
    ///   makakoo bucket revoke   <grant-id>
    ///   makakoo bucket expire                  -- run TTL purge now
    ///   makakoo bucket deny-all <name>  [--ttl <dur>]
    Bucket {
        #[command(subcommand)]
        cmd: BucketCmd,
    },

    /// Docs MCP server — Makakoo OS documentation over MCP/stdio.
    ///
    /// Run with `--stdio` to serve `makakoo_docs_search / read / list / topic`
    /// tools over JSON-RPC on stdin/stdout. Wire into your AI CLI's MCP
    /// config — see docs/docs-mcp-setup.md.
    ///
    ///   makakoo docs-mcp --stdio
    DocsMcp {
        /// Run as a stdio JSON-RPC MCP server. Required.
        #[arg(long)]
        stdio: bool,
    },

    /// Docs corpus management.
    ///
    /// Subcommands:
    ///   makakoo docs update [--from-github] [--from-branch <branch>]
    ///
    /// `update` fetches the latest `docs/` + `spec/` from GitHub,
    /// rebuilds the FTS5 index, and writes it to
    /// `~/.makakoo/docs-cache/index.db`. The MCP server prefers this
    /// cache over the baked-in corpus on next start.
    Docs {
        #[command(subcommand)]
        cmd: DocsCmd,
    },
}

/// `makakoo docs <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum DocsCmd {
    /// Fetch the latest docs from GitHub and rebuild the local FTS5
    /// cache at `~/.makakoo/docs-cache/index.db`.
    Update {
        /// Fetch from `github.com/makakoo/makakoo-os` (default behaviour;
        /// currently the only supported source).
        #[arg(long, default_value_t = true)]
        from_github: bool,

        /// Override the branch to fetch (default: `main`).
        #[arg(long, value_name = "BRANCH")]
        from_branch: Option<String>,
    },
}

/// `makakoo s3 <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum S3Cmd {
    /// Bootstrap the `makakoo-s3-service` keypair against the local
    /// Garage admin API. Idempotent — re-running is a no-op when the
    /// keypair already exists in the keychain. Designed to be called
    /// by `plugins-core/garage-store/bin/garage-wrapper.sh` as a
    /// fire-and-forget after `garage server` reaches ready state, and
    /// by operators who need the key present before Phase C bucket ops.
    Bootstrap {
        /// Delete the existing keypair (in Garage + keychain) and
        /// generate a fresh one. Emits `s3.service_key_rotated` event.
        #[arg(long)]
        force_rotate: bool,
    },

    /// Manage the multi-backend S3 endpoint registry —
    /// `$MAKAKOO_HOME/config/s3_endpoints.json` + per-endpoint
    /// credentials in the OS keychain.
    Endpoint {
        #[command(subcommand)]
        cmd: S3EndpointCmd,
    },
}

/// `makakoo s3 endpoint <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum S3EndpointCmd {
    /// List every registered endpoint. Default endpoint marked with `*`.
    List {
        /// Emit JSON instead of the default table.
        #[arg(long)]
        json: bool,
    },

    /// Register a new endpoint and store its credentials in the keychain.
    Add {
        /// Endpoint name — referenced by `--endpoint` flags later.
        name: String,
        /// Endpoint URL (e.g. `https://s3.amazonaws.com`).
        #[arg(long)]
        url: String,
        /// AWS region string (e.g. `us-east-1` for AWS, `garage` for
        /// the local Garage backend).
        #[arg(long)]
        region: String,
        /// Backend kind. Drives backend-specific quirks Phase C wires up.
        #[arg(long, value_parser = ["garage-local", "aws", "r2", "b2", "minio"])]
        kind: String,
        /// Access key ID.
        #[arg(long)]
        access_key: String,
        /// Secret access key.
        #[arg(long)]
        secret_key: String,
        /// Permit JSON-file fallback when keychain write fails. Without
        /// this flag, a keychain failure refuses the operation rather
        /// than silently downgrading to plaintext-on-disk creds.
        #[arg(long)]
        allow_file_creds: bool,
    },

    /// Remove an endpoint. Wipes its keychain entry too.
    Remove {
        /// Endpoint name.
        name: String,
    },

    /// Set which endpoint is used when `--endpoint` is omitted.
    Default {
        /// Endpoint name.
        name: String,
    },

    /// Health-probe an endpoint by attempting `ListBuckets`. Reports
    /// OK / auth-fail / network-fail / endpoint-404.
    Test {
        /// Endpoint name (defaults to the registered default).
        name: Option<String>,
    },
}

/// `makakoo bucket <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum BucketCmd {
    /// Create a new bucket on the chosen backend (default: local Garage).
    /// Default TTL is 7 days; default quota is 10 GB. Pass
    /// `--ttl permanent` or `--quota unlimited` with `--confirm-yes-really`
    /// to override.
    Create {
        /// Bucket name. 3–63 chars, lowercase letters / digits / dot /
        /// hyphen only; must start + end with alphanumeric. No
        /// underscores. Validated Makakoo-side BEFORE backend dispatch.
        name: String,

        /// Backend endpoint name (defaults to registry default).
        /// Garage-only in v0.7.1; non-Garage backends raise
        /// `NotImplementedError`-equivalent CLI errors with v0.8 pointer.
        #[arg(long)]
        endpoint: Option<String>,

        /// TTL — `30m | 1h | 24h | 7d | permanent`. Default `7d`.
        #[arg(long, default_value = "7d")]
        ttl: String,

        /// Hard quota — e.g. `100M`, `1G`, `10G`, or `unlimited`.
        /// Default `10G`.
        #[arg(long, default_value = "10G")]
        quota: String,

        /// Required to use `--ttl permanent` or `--quota unlimited`.
        #[arg(long)]
        confirm_yes_really: bool,
    },

    /// List buckets known to Makakoo on the chosen backend.
    List {
        /// Backend endpoint name (default: every registered endpoint).
        #[arg(long)]
        endpoint: Option<String>,
        /// Emit JSON instead of the default table.
        #[arg(long)]
        json: bool,
    },

    /// Show one bucket's metadata (TTL, quota, usage %, grants).
    Info {
        /// Bucket name.
        name: String,
        /// Emit JSON instead of the default human view.
        #[arg(long)]
        json: bool,
    },

    /// Grant a per-bucket scoped sub-keypair to a labeled consumer.
    /// Returns `(endpoint_url, access_key, secret_key, expires_at)` on
    /// stdout; the caller wires these into their own boto3 / aws-cli /
    /// rclone config.
    Grant {
        /// Bucket name.
        bucket: String,
        /// Human-readable label for the grantee — appears in
        /// `makakoo perms list` and audit log.
        #[arg(long)]
        to: String,
        /// Comma-separated permission set: `read`, `read,write`, or
        /// `read,write,owner`.
        #[arg(long, default_value = "read,write")]
        perms: String,
        /// TTL — `30m | 1h | 24h | 7d | permanent`. Default `1h`.
        #[arg(long, default_value = "1h")]
        ttl: String,
        /// Required to use `--ttl permanent`.
        #[arg(long)]
        confirm_yes_really: bool,
        /// Emit JSON instead of the default human view.
        #[arg(long)]
        json: bool,
    },

    /// Revoke a bucket grant by its ID. Atomic 3-state transition:
    /// `active → revoking → revoked`. SANCHO retries the backend
    /// delete every 60s if the first attempt fails (lope-1, qwen).
    Revoke {
        /// Grant ID (as printed by `bucket grant` or `perms list`).
        grant_id: String,
    },

    /// Run the SANCHO `bucket-expire` task once, synchronously. Walks
    /// the bucket registry, purges TTL'd buckets and TTL'd grants.
    Expire {
        /// Don't actually delete anything — just print what would happen.
        #[arg(long)]
        dry_run: bool,
    },

    /// Emergency stop: flip a bucket flag that makes Garage 403 every
    /// read/write, including those carrying a still-valid presigned URL.
    /// Mirrors the `LD#12 path 2` revocation semantics.
    DenyAll {
        /// Bucket name.
        name: String,
        /// TTL — flag clears automatically after this duration. Default
        /// `1h`. `--ttl permanent` requires `--confirm-yes-really`.
        #[arg(long, default_value = "1h")]
        ttl: String,
        /// Required to use `--ttl permanent`.
        #[arg(long)]
        confirm_yes_really: bool,
    },
}

/// `makakoo agent <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Run the plugin's `[entrypoint].start` script.
    Start {
        /// Plugin name (as reported by `makakoo plugin list`).
        name: String,
    },
    /// Run the plugin's `[entrypoint].stop` script.
    Stop {
        /// Plugin name.
        name: String,
    },
    /// Show whether the plugin's agent process is running. Uses the
    /// plugin's `[entrypoint].health` script if declared, else pgrep.
    Status {
        /// Plugin name.
        name: String,
    },
    /// Run the plugin's `[entrypoint].health` script (exits 0 if up).
    Health {
        /// Plugin name.
        name: String,
    },

    // ── Multi-bot subagent registry (Phase 2) ─────────────────────
    /// List every configured slot in `~/MAKAKOO/config/agents/*.toml`.
    List {
        /// Emit JSON instead of the human table.
        #[arg(long)]
        json: bool,
    },
    /// Print the resolved TOML for one slot, with secret fields redacted.
    Show {
        /// Slot id (matches the TOML filename stem).
        slot: String,
        /// Emit JSON instead of TOML.
        #[arg(long)]
        json: bool,
    },
    /// Run per-transport credential verifiers WITHOUT starting the
    /// agent process. Useful before `start` to surface bad credentials.
    Validate {
        /// Slot id.
        slot: String,
    },
    /// Inventory existing `agent-*` plugins with their migration
    /// status (active / migrated / pending). Does NOT migrate them.
    Inventory {
        /// Emit JSON instead of the human table.
        #[arg(long)]
        json: bool,
    },
    /// Create a new slot from flags (single Telegram, single Slack,
    /// or `--from-toml` for arbitrary multi-transport configs).
    Create {
        /// Slot id (also the TOML filename).
        slot: String,
        /// Display name shown in `agent list` Name column. Defaults
        /// to the slot id if unset.
        #[arg(long)]
        name: Option<String>,
        /// Per-agent persona snippet. Use `null` to inherit the
        /// canonical bootstrap (HARVEY_SYSTEM_PROMPT) — same as
        /// omitting the flag.
        #[arg(long)]
        persona: Option<String>,
        /// Allowed filesystem paths (comma-separated). The slot's
        /// scope for read/write tool access.
        #[arg(long, value_name = "PATHS", value_delimiter = ',')]
        allowed_paths: Vec<String>,
        /// Forbidden paths (comma-separated). Overrides
        /// allowed_paths.
        #[arg(long, value_name = "PATHS", value_delimiter = ',')]
        forbidden_paths: Vec<String>,
        /// Tool whitelist (comma-separated tool names).
        #[arg(long, value_name = "TOOLS", value_delimiter = ',')]
        tools: Vec<String>,
        /// Path to a TOML file pre-built by the operator (multi-
        /// transport configs). Mutually exclusive with --telegram-token
        /// and --slack-* flags.
        #[arg(long, value_name = "PATH")]
        from_toml: Option<std::path::PathBuf>,
        /// Telegram bot token. Triggers single-Telegram-transport
        /// mode. The token is stored as inline_secret_dev — for
        /// production move it to env or makakoo secret.
        #[arg(long, value_name = "TOKEN")]
        telegram_token: Option<String>,
        /// Telegram allowed_users (comma-separated chat_ids).
        #[arg(long, value_name = "IDS", value_delimiter = ',')]
        telegram_allowed: Vec<String>,
        /// Slack bot token (`xoxb-…`). Triggers single-Slack-
        /// transport mode. Requires --slack-app-token + --slack-team.
        #[arg(long, value_name = "TOKEN")]
        slack_bot_token: Option<String>,
        /// Slack app token (`xapp-…`).
        #[arg(long, value_name = "TOKEN")]
        slack_app_token: Option<String>,
        /// Slack team_id (`T0123ABCD`).
        #[arg(long, value_name = "TEAM")]
        slack_team: Option<String>,
        /// Slack allowed_users (comma-separated `U…` ids).
        #[arg(long, value_name = "USERS", value_delimiter = ',')]
        slack_allowed: Vec<String>,
        /// Skip the `getMe` / `auth.test` credential probe.  Use
        /// only for offline scaffold of a slot whose tokens you'll
        /// fix up afterward.
        #[arg(long)]
        skip_credential_check: bool,
    },

    /// Migrate the legacy HarveyChat (`Olibia`) bot from
    /// `data/chat/config.json` to a `harveychat` subagent slot.
    /// Idempotent: re-running on an already-migrated slot is a no-op.
    MigrateHarveychat,

    /// Restart a slot's supervisor (= stop then start).
    Restart {
        /// Slot id (or legacy plugin name).
        name: String,
    },

    /// Destroy a slot interactively. Stops the supervisor (if
    /// running), archives the TOML + data dir to
    /// `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/`, and lists
    /// any direct `secret_ref = "..."` literals found in the TOML
    /// (the operator decides whether to revoke them via the
    /// separate `--revoke-secrets` flag).
    Destroy {
        /// Slot id to destroy.
        slot: String,
        /// Skip the destroy confirmation prompt. Does NOT
        /// auto-revoke secrets.
        #[arg(long)]
        yes: bool,
        /// Also revoke detected secrets from the keyring after a
        /// successful destroy. Off by default.
        #[arg(long)]
        revoke_secrets: bool,
        /// No-op flag accepted for explicit clarity (secrets are
        /// preserved by default already).
        #[arg(long)]
        keep_secrets: bool,
        /// Required to destroy the legacy `harveychat` slot. Without
        /// it, attempting to destroy `harveychat` is rejected to
        /// protect the legacy Olibia conversation history.
        #[arg(long)]
        really_destroy_harveychat: bool,
    },

    /// Tail the per-machine audit log (Phase 12). Surface scope
    /// violations, secret resolutions, slot lifecycle, webhook
    /// signature failures, fault tests, etc.
    Audit {
        /// Number of most-recent events to show. Default 50.
        #[arg(long, default_value_t = 50)]
        last: usize,
        /// Filter to a single audit kind (e.g. `scope_tool`,
        /// `webhook_invalid_signature`, `rate_limit`).
        #[arg(long)]
        kind: Option<String>,
        /// Emit JSON lines instead of the human table.
        #[arg(long)]
        json: bool,
    },

    /// Run the fault-injection scenario suite (Phase 12 / Q11).
    /// Gated behind `MAKAKOO_DEV_FAULTS=1` so production cannot
    /// trigger. All scenarios are mock-only — no real transport
    /// credentials, no network calls.
    TestFaults {
        /// Run a single scenario by name (kebab-case, e.g.
        /// `gateway-sigterm`). Default: run the entire suite.
        #[arg(long)]
        scenario: Option<String>,
        /// Emit JSON lines instead of the human report.
        #[arg(long)]
        json: bool,
    },

    // ── Internal: invoked by launchd / systemd, NOT for direct use. ──
    /// Internal: the long-running per-slot supervisor process. The
    /// LaunchAgent plist / systemd unit invokes this. Users should
    /// never run it directly.
    #[command(name = "_supervisor", hide = true)]
    Supervisor {
        /// Slot id to supervise.
        #[arg(long)]
        slot: String,
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

    /// Install a plugin from a local path, a git URL, or an HTTPS tarball.
    ///
    /// Accepted `<source>` shapes:
    ///   - `path/to/dir`                      — local directory (or use `--core`)
    ///   - `git+<url>[@<ref>]`                — git repo pinned to tag or 40-char SHA
    ///   - `https://.../x.tar.gz`             — tarball (requires `--sha256`)
    ///   - bare name + `--core`               — resolves against `plugins-core/`
    Install {
        /// Source — see shapes above.
        source: String,

        /// Resolve `source` against `$MAKAKOO_PLUGINS_CORE` (or the
        /// `plugins-core/` dir under the current repo).
        #[arg(long)]
        core: bool,

        /// Expected blake3 of the plugin source tree. Takes precedence
        /// over the value declared in the manifest.
        #[arg(long)]
        blake3: Option<String>,

        /// Expected sha256 of the tarball bytes. Required for tarball
        /// sources (`https://...`). Ignored for path and git sources.
        #[arg(long)]
        sha256: Option<String>,

        /// Permit non-tag-non-SHA git refs (e.g. `main`, `master`,
        /// branch names). Without this flag, git+<url>@<ref> requires
        /// the ref to be a semver tag or 40-char SHA.
        #[arg(long)]
        allow_unstable_ref: bool,
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
    /// Path-sourced plugins: uninstall + reinstall from the recorded
    /// directory. Git-sourced plugins: refetch upstream ref, diff manifest
    /// hash, prompt on capability drift (override with `--yes`), then
    /// reinstall. Tarball-sourced plugins: surface hint to reinstall
    /// with a fresh `--sha256` (v0.4 restricts tarball auto-update).
    /// Preserves the plugin's enabled / disabled flag across the
    /// reinstall. State directories are preserved (no `--purge`).
    Update {
        /// Plugin name. Required unless `--all` is set.
        #[arg(required_unless_present = "all")]
        name: Option<String>,

        /// Update every updatable (git + tarball) plugin. Per-plugin
        /// failures log + skip; the batch continues.
        #[arg(long)]
        all: bool,

        /// Skip the manifest-drift re-trust prompt. Dangerous — only
        /// use when you trust upstream unconditionally.
        #[arg(long)]
        yes: bool,
    },

    /// List every updatable plugin whose upstream ref has drifted. Pure
    /// dry-run; no disk state is mutated.
    Outdated {
        /// Emit JSON instead of the default table.
        #[arg(long)]
        json: bool,
    },

    /// Start a service-kind or agent-kind plugin.
    ///
    /// Service plugins: backgrounded, stdout/stderr redirected to
    /// `~/Library/Logs/makakoo/<plugin>.{out,err}.log` (macOS) or
    /// `~/.local/state/makakoo/log/` (Linux).
    /// Agent plugins: foreground exec — same as `makakoo agent start`.
    Start {
        /// Plugin name.
        name: String,
    },

    /// Stop a service-kind or agent-kind plugin.
    Stop {
        /// Plugin name.
        name: String,
    },

    /// Probe a service-kind or agent-kind plugin's health.
    /// Service plugins: probes `[service].health_endpoint` (HTTP if URL,
    /// otherwise shell) or falls back to `[entrypoint].health`.
    /// Agent plugins: same fallback chain as `makakoo agent status`.
    Status {
        /// Plugin name.
        name: String,
    },

    /// Stop then start a service-kind or agent-kind plugin.
    Restart {
        /// Plugin name.
        name: String,
    },

    /// Hidden: kernel-internal helpers a plugin's install.sh can invoke.
    /// Exposed for `makakoo-venv-bootstrap` — not part of the public
    /// CLI contract.
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        cmd: PluginInternalCmd,
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

/// Hidden plugin-internal subcommands. Stable wire contract for shell
/// helpers like `makakoo-venv-bootstrap`; not documented publicly.
#[derive(Subcommand, Debug)]
pub enum PluginInternalCmd {
    /// Create + populate a per-plugin Python venv. Reads the target
    /// directory from `$MAKAKOO_PLUGIN_DIR` (set by the installer when
    /// it invokes `[install].unix`).
    VenvBootstrap {
        /// `editable` (default) | `pip` | `git`. `editable` runs
        /// `pip install -e .` against the plugin dir. `pip` requires
        /// `--spec`. `git` requires `--url` (optionally `--rev`).
        #[arg(long, default_value = "editable")]
        mode: String,

        /// Raw pip spec — only meaningful with `--mode pip`.
        /// e.g. `-r requirements.txt` or `requests==2.31`.
        #[arg(long)]
        spec: Option<String>,

        /// Git URL — only meaningful with `--mode git`.
        #[arg(long)]
        url: Option<String>,

        /// Git ref (tag or 40-char SHA) — only meaningful with `--mode git`.
        #[arg(long)]
        rev: Option<String>,

        /// Python binary (default `python3`).
        #[arg(long, default_value = "python3")]
        python: String,
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

/// `makakoo perms <subcommand>`. Manages the runtime user-grant layer
/// (`$MAKAKOO_HOME/config/user_grants.json`). See `spec/USER_GRANTS.md`
/// for the file format and `spec/CAPABILITIES.md §1.11` for the
/// three-layer model.
#[derive(Subcommand, Debug)]
pub enum PermsCmd {
    /// List active grants (omit `--all` to hide expired).
    List {
        /// Emit JSON instead of a human table.
        #[arg(long)]
        json: bool,
        /// Include expired grants in the output.
        #[arg(long)]
        all: bool,
    },

    /// Issue a new write grant. Default duration is 1 hour per LD#11.
    ///
    /// Scope refusal: `/`, `~`, `~/`, `$HOME`, empty, bare `*`, bare
    /// `**` are rejected at this handler regardless of who asks.
    /// Permanent grants outside `$MAKAKOO_HOME` require `--yes-really`.
    Grant {
        /// Path to grant write access to. `~` and `$VAR` expand at
        /// grant-time (not check-time).
        path: String,
        /// Duration: `30m` | `1h` | `24h` | `7d` | `permanent`.
        /// Natural-language phrases ("for an hour") are rejected in
        /// v1 per lope F12 / LD#15 — deferred to v0.3.1.
        #[arg(long = "for", default_value = "1h")]
        duration: String,
        /// Free-text label (≤ 80 chars; control chars stripped).
        #[arg(long)]
        label: Option<String>,
        /// Caller-surface attribution. Defaults to `cli`.
        #[arg(long, default_value = "cli")]
        plugin: String,
        /// Create the target directory if it doesn't exist.
        #[arg(long)]
        mkdir: bool,
        /// Confirm a permanent grant outside `$MAKAKOO_HOME`.
        #[arg(long = "yes-really")]
        yes_really: bool,
    },

    /// Revoke a grant by id, or by unambiguous path.
    Revoke {
        /// Grant id (e.g. `g_20260421_abcd1234`).
        id: Option<String>,
        /// Alternative: revoke by scope path. Must match exactly
        /// one active grant.
        #[arg(long)]
        path: Option<String>,
        /// Emit JSON confirmation.
        #[arg(long)]
        json: bool,
    },

    /// Drop expired grants from the store. Writes a `perms/revoke`
    /// audit entry per expired grant (reason=`expired`).
    Purge {
        /// Emit JSON (list of removed grant ids).
        #[arg(long)]
        json: bool,
    },

    /// Show recent audit entries for `perms/*` and `fs/write` verbs.
    Audit {
        /// Only entries newer than this duration (e.g. `1h`, `24h`, `7d`).
        #[arg(long)]
        since: Option<String>,
        /// Filter by plugin attribution.
        #[arg(long)]
        plugin: Option<String>,
        /// Filter by grant id (matches `scope_granted`).
        #[arg(long)]
        grant: Option<String>,
        /// Emit JSON (one entry per array element).
        #[arg(long)]
        json: bool,
    },

    /// Show detail for one grant by id.
    Show {
        /// Grant id.
        id: String,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
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

    /// Call a registered adapter with a prompt. Reads prompt from stdin by
    /// default, or pass via `--prompt`. Writes a single JSON
    /// ValidatorResult to stdout — the same shape lope's Python
    /// `PhaseVerdict` + `ValidatorResult` dataclasses hydrate from. This
    /// is the interop seam lope's GenericAdapterValidator shells into.
    Call {
        /// Adapter name as registered (or bundled via `--bundled`).
        name: String,
        /// Provide the prompt inline instead of reading stdin.
        #[arg(long)]
        prompt: Option<String>,
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Resolve the adapter from `plugins-core/adapters/` in addition
        /// to the registered dir.
        #[arg(long)]
        bundled: bool,
    },

    /// Install an adapter. `<source>` is either a local directory
    /// containing `adapter.toml` or a bundled reference adapter name
    /// (with `--bundled`). URL installs (git / https-tarball / pypi /
    /// npm) ship after Phase D. `--pack` treats `<source>` as a pack
    /// root and installs every `<subdir>/adapter.toml` under it.
    Install {
        /// Path to a local adapter dir, or a bundled adapter name.
        source: String,
        /// Treat `source` as a bundled reference adapter name.
        #[arg(long)]
        bundled: bool,
        /// Treat `source` as an adapters-core-style pack: walk every
        /// `<subdir>/adapter.toml` under it and install each.
        #[arg(long)]
        pack: bool,
        /// Allow unsigned URL installs (local paths are always allowed).
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept the capability diff without the interactive prompt
        /// (used for scripted re-trusts).
        #[arg(long)]
        accept_re_trust: bool,
        /// Skip the install-time health check (dev loop only).
        #[arg(long)]
        skip_health_check: bool,
    },

    /// Re-run install against the currently-registered adapter's source.
    /// Detects capability / security drift and prompts (or honors
    /// --accept-re-trust).
    Update {
        /// Adapter name.
        name: String,
        /// Accept the diff without prompt.
        #[arg(long)]
        accept_re_trust: bool,
    },

    /// Remove a registered adapter. Clears the trust entry. With
    /// `--purge`, also wipes the adapter's state dir under
    /// `~/.makakoo/adapters/state/<name>/`.
    Remove {
        /// Adapter name.
        name: String,
        /// Also delete the adapter's state dir.
        #[arg(long)]
        purge: bool,
    },

    /// Enable a previously-disabled adapter (soft toggle — manifest
    /// stays on disk, the `disabled` marker is dropped).
    Enable { name: String },

    /// Disable an adapter without uninstalling it. Consumers (lope,
    /// swarm, chat) skip disabled adapters on their next registry read.
    Disable { name: String },

    /// Show a status table for every registered adapter: last call
    /// outcome, last call timestamp, last error.
    Status {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Diagnose an adapter — env presence, auth smoke, health-check,
    /// signature verify. Each check reports ✅ or ❌ with a remediation
    /// hint.
    Doctor {
        /// Adapter name.
        name: String,
        /// Emit JSON instead of the human-readable table.
        #[arg(long)]
        json: bool,
    },

    /// Fuzzy name filter across registered + bundled adapters.
    Search {
        /// Free-form substring query.
        query: String,
    },

    /// Migrate legacy lope-config providers → adapter manifests.
    /// Reads the given `~/.lope/config.json` (or equivalent), emits one
    /// `.toml` per provider entry into the registered dir.
    MigrateConfig {
        /// Path to a lope config.json file with a `providers` array.
        #[arg(value_name = "PATH")]
        path: std::path::PathBuf,
    },

    /// Dump a registered adapter's manifest as a signed tarball
    /// (adapter.toml + adapter.toml.sig). When `--sign` is omitted, the
    /// tarball contains only the manifest.
    Export {
        /// Adapter name.
        name: String,
        /// Output path (defaults to `./<name>.tar.gz`).
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },

    /// v0.6 — manage the per-peer trust store used by signed HTTP MCP
    /// (`$MAKAKOO_HOME/config/peers/trusted.keys`). Each line names a
    /// peer Makakoo install authorized to reach this one over HTTP.
    Trust {
        #[command(subcommand)]
        cmd: AdapterTrustCmd,
    },

    /// v0.6 — print this install's Ed25519 public key for peers to add
    /// to their trust files. Generates the keypair on first invocation.
    SelfPubkey {
        /// Also print the fingerprint alongside the full base64 key.
        #[arg(long)]
        with_fingerprint: bool,
    },

    /// v0.6 — scaffold a new adapter from a template. Writes an
    /// adapter.toml with the appropriate shape, installs it to the
    /// registered dir, and (unless --skip-doctor) runs `adapter doctor`
    /// on the result.
    Gen {
        /// Template shape: `openai-compat` | `subprocess` | `mcp-stdio`
        /// | `peer-makakoo`.
        #[arg(long)]
        template: String,
        /// Adapter name (lowercase, hyphens allowed).
        #[arg(long)]
        name: String,
        /// Free-form description (defaults to a template-specific string).
        #[arg(long)]
        description: Option<String>,
        /// Base URL (required for openai-compat / peer-makakoo).
        #[arg(long)]
        url: Option<String>,
        /// Env var name holding the API bearer token (openai-compat).
        /// Default: `<NAME>_API_KEY` with hyphens → underscores, uppercased.
        #[arg(long)]
        key_env: Option<String>,
        /// Model name to send in chat completions (openai-compat).
        #[arg(long)]
        model: Option<String>,
        /// Command argv (required for subprocess / mcp-stdio). Each
        /// occurrence adds one element: `--command bash --command -c`.
        #[arg(long)]
        command: Vec<String>,
        /// Adapter roles (comma-separated). Default `delegate,swarm_member`.
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Peer name the remote Makakoo install knows us by (required
        /// for peer-makakoo template).
        #[arg(long)]
        peer_name: Option<String>,
        /// Skip the post-gen `doctor` call (useful if the remote isn't
        /// reachable from this machine yet).
        #[arg(long)]
        skip_doctor: bool,
        /// Skip the post-gen install entirely — just render the manifest
        /// to the scratch dir and print the path.
        #[arg(long)]
        skip_install: bool,
        /// Skip the install-time health check (dev loop).
        #[arg(long)]
        skip_health_check: bool,
    },
}

/// v0.6 — peer trust subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum AdapterTrustCmd {
    /// Add or replace a peer → pubkey entry.
    Add {
        /// Peer name (how this install will refer to the remote).
        name: String,
        /// Remote's Ed25519 pubkey (base64, 32 bytes decoded).
        pubkey: String,
    },
    /// List trusted peers. Prints fingerprints by default; pass
    /// `--with-keys` to include full base64 pubkeys.
    List {
        /// Include full base64 pubkey in output.
        #[arg(long)]
        with_keys: bool,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove a peer by name. Silent no-op if the peer isn't in the
    /// trust file.
    Remove {
        /// Peer name.
        name: String,
    },
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
        match cli.command.unwrap() {
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
        if let Commands::Search { query, limit } = cli.command.unwrap() {
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
        if let Commands::Query { question, top_k, .. } = cli.command.unwrap() {
            assert_eq!(question, "what is lope?");
            assert_eq!(top_k, 3);
        } else {
            panic!("expected Query");
        }
    }

    #[test]
    fn parse_sancho_tick() {
        let cli = Cli::try_parse_from(["makakoo", "sancho", "tick"]).unwrap();
        matches!(cli.command.unwrap(), Commands::Sancho { cmd: SanchoCmd::Tick });
    }

    #[test]
    fn parse_sancho_status() {
        let cli = Cli::try_parse_from(["makakoo", "sancho", "status"]).unwrap();
        if let Commands::Sancho { cmd } = cli.command.unwrap() {
            matches!(cmd, SanchoCmd::Status);
        } else {
            panic!("expected Sancho");
        }
    }

    #[test]
    fn parse_buddy_status() {
        let cli = Cli::try_parse_from(["makakoo", "buddy", "status"]).unwrap();
        if let Commands::Buddy { cmd } = cli.command.unwrap() {
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
        } = cli.command.unwrap()
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
        if let Commands::Nursery { cmd } = cli.command.unwrap() {
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
        if let Commands::Skill { name, args } = cli.command.unwrap() {
            assert_eq!(name, "canary");
            assert_eq!(args, vec!["run", "opencode", "--workspace", "clean"]);
        } else {
            panic!("expected Skill");
        }
    }

    #[test]
    fn parse_dream() {
        let cli = Cli::try_parse_from(["makakoo", "dream"]).unwrap();
        matches!(cli.command.unwrap(), Commands::Dream);
    }

    #[test]
    fn parse_promotions_defaults() {
        let cli = Cli::try_parse_from(["makakoo", "promotions"]).unwrap();
        if let Commands::Promotions { threshold, limit } = cli.command.unwrap() {
            assert!((threshold - 0.70).abs() < 1e-6);
            assert_eq!(limit, 10);
        } else {
            panic!("expected Promotions");
        }
    }

    #[test]
    fn parse_version() {
        let cli = Cli::try_parse_from(["makakoo", "version"]).unwrap();
        matches!(cli.command.unwrap(), Commands::Version);
    }

    #[test]
    fn parse_mcp_with_passthrough_args() {
        let cli = Cli::try_parse_from(["makakoo", "mcp", "--list-tools"]).unwrap();
        if let Commands::Mcp { args } = cli.command.unwrap() {
            assert_eq!(args, vec!["--list-tools"]);
        } else {
            panic!("expected Mcp");
        }
    }

    #[test]
    fn parse_plugin_list() {
        let cli = Cli::try_parse_from(["makakoo", "plugin", "list"]).unwrap();
        match cli.command.unwrap() {
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
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
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
                    ..
                },
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
        {
            assert_eq!(source, "/tmp/my-plugin");
            assert!(!core);
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_install_git_url_with_tag() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "install",
            "git+https://github.com/user/plugin@v0.1.0",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd:
                PluginCmd::Install {
                    source,
                    allow_unstable_ref,
                    ..
                },
        } = cli.command.unwrap()
        {
            assert_eq!(source, "git+https://github.com/user/plugin@v0.1.0");
            assert!(!allow_unstable_ref);
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_install_git_with_allow_unstable_ref() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "install",
            "git+https://github.com/user/plugin@main",
            "--allow-unstable-ref",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd:
                PluginCmd::Install {
                    allow_unstable_ref,
                    ..
                },
        } = cli.command.unwrap()
        {
            assert!(allow_unstable_ref);
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_install_tarball_with_sha256() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "install",
            "https://example.com/plugin.tar.gz",
            "--sha256",
            &"a".repeat(64),
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::Install { source, sha256, .. },
        } = cli.command.unwrap()
        {
            assert!(source.starts_with("https://"));
            assert_eq!(sha256.unwrap().len(), 64);
        } else {
            panic!("expected Plugin::Install");
        }
    }

    #[test]
    fn parse_plugin_internal_venv_bootstrap_editable() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "internal",
            "venv-bootstrap",
            "--mode",
            "editable",
        ])
        .unwrap();
        let Commands::Plugin {
            cmd:
                PluginCmd::Internal {
                    cmd: PluginInternalCmd::VenvBootstrap { mode, .. },
                },
        } = cli.command.unwrap()
        else {
            panic!("expected Plugin::Internal::VenvBootstrap");
        };
        assert_eq!(mode, "editable");
    }

    #[test]
    fn parse_plugin_internal_venv_bootstrap_git_with_rev() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "plugin",
            "internal",
            "venv-bootstrap",
            "--mode",
            "git",
            "--url",
            "https://github.com/x/y",
            "--rev",
            "v1.2.3",
        ])
        .unwrap();
        let Commands::Plugin {
            cmd:
                PluginCmd::Internal {
                    cmd:
                        PluginInternalCmd::VenvBootstrap {
                            mode, url, rev, ..
                        },
                },
        } = cli.command.unwrap()
        else {
            panic!("expected Plugin::Internal::VenvBootstrap");
        };
        assert_eq!(mode, "git");
        assert_eq!(url.as_deref(), Some("https://github.com/x/y"));
        assert_eq!(rev.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn parse_plugin_uninstall_with_purge() {
        let cli = Cli::try_parse_from([
            "makakoo", "plugin", "uninstall", "mascot-gym", "--purge",
        ])
        .unwrap();
        if let Commands::Plugin {
            cmd: PluginCmd::Uninstall { name, purge },
        } = cli.command.unwrap()
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
            cli.command.unwrap(),
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
        } = cli.command.unwrap()
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
            no_setup,
        } = cli.command.unwrap()
        {
            assert_eq!(distro, "core");
            assert!(!dry_run);
            assert!(!yes);
            assert!(!skip_daemon);
            assert!(!skip_infect);
            assert!(!no_setup);
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
        } = cli.command.unwrap()
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
        match cli.command.unwrap() {
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
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
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
        } = cli.command.unwrap()
        {
            assert!(name.is_none());
            assert_eq!(from.as_deref().map(|p| p.to_str().unwrap()), Some("/tmp/custom.toml"));
            assert!(dry_run);
        } else {
            panic!("expected Distro::Install");
        }
    }

    // ───────────────────────── Adapter subcommand parsing ──────────────────────────

    #[test]
    fn parse_adapter_list() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "list"]).unwrap();
        match cli.command.unwrap() {
            Commands::Adapter {
                cmd:
                    AdapterCmd::List {
                        json: false,
                        include_bundled: false,
                    },
            } => {}
            _ => panic!("expected Adapter::List"),
        }
    }

    #[test]
    fn parse_adapter_list_with_flags() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "list",
            "--json",
            "--include-bundled",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd:
                AdapterCmd::List {
                    json,
                    include_bundled,
                },
        } = cli.command.unwrap()
        {
            assert!(json);
            assert!(include_bundled);
        } else {
            panic!("expected Adapter::List");
        }
    }

    #[test]
    fn parse_adapter_install_bundled() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "install",
            "openclaw",
            "--bundled",
            "--skip-health-check",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd:
                AdapterCmd::Install {
                    source,
                    bundled,
                    pack,
                    allow_unsigned,
                    accept_re_trust,
                    skip_health_check,
                },
        } = cli.command.unwrap()
        {
            assert_eq!(source, "openclaw");
            assert!(bundled);
            assert!(!pack);
            assert!(!allow_unsigned);
            assert!(!accept_re_trust);
            assert!(skip_health_check);
        } else {
            panic!("expected Adapter::Install");
        }
    }

    #[test]
    fn parse_adapter_install_pack() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "install",
            "./adapters-core",
            "--pack",
            "--skip-health-check",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Install { pack, .. },
        } = cli.command.unwrap()
        {
            assert!(pack);
        } else {
            panic!("expected Adapter::Install");
        }
    }

    #[test]
    fn parse_adapter_install_allow_unsigned() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "install",
            "./my-adapter",
            "--allow-unsigned",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Install { allow_unsigned, .. },
        } = cli.command.unwrap()
        {
            assert!(allow_unsigned);
        } else {
            panic!("expected Adapter::Install");
        }
    }

    #[test]
    fn parse_adapter_update() {
        let cli =
            Cli::try_parse_from(["makakoo", "adapter", "update", "openclaw", "--accept-re-trust"])
                .unwrap();
        if let Commands::Adapter {
            cmd:
                AdapterCmd::Update {
                    name,
                    accept_re_trust,
                },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert!(accept_re_trust);
        } else {
            panic!("expected Adapter::Update");
        }
    }

    #[test]
    fn parse_adapter_remove_with_purge() {
        let cli =
            Cli::try_parse_from(["makakoo", "adapter", "remove", "openclaw", "--purge"]).unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Remove { name, purge },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert!(purge);
        } else {
            panic!("expected Adapter::Remove");
        }
    }

    #[test]
    fn parse_adapter_enable_disable() {
        let e = Cli::try_parse_from(["makakoo", "adapter", "enable", "foo"]).unwrap();
        matches!(
            e.command.unwrap(),
            Commands::Adapter {
                cmd: AdapterCmd::Enable { .. },
            }
        );
        let d = Cli::try_parse_from(["makakoo", "adapter", "disable", "foo"]).unwrap();
        matches!(
            d.command.unwrap(),
            Commands::Adapter {
                cmd: AdapterCmd::Disable { .. },
            }
        );
    }

    #[test]
    fn parse_adapter_status_json() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "status", "--json"]).unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Status { json },
        } = cli.command.unwrap()
        {
            assert!(json);
        } else {
            panic!("expected Adapter::Status");
        }
    }

    #[test]
    fn parse_adapter_doctor() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "doctor", "openclaw"]).unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Doctor { name, json },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert!(!json);
        } else {
            panic!("expected Adapter::Doctor");
        }
    }

    #[test]
    fn parse_adapter_search() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "search", "claw"]).unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Search { query },
        } = cli.command.unwrap()
        {
            assert_eq!(query, "claw");
        } else {
            panic!("expected Adapter::Search");
        }
    }

    #[test]
    fn parse_adapter_migrate_config() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "migrate-config",
            "/home/me/.lope/config.json",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::MigrateConfig { path },
        } = cli.command.unwrap()
        {
            assert_eq!(path.to_str().unwrap(), "/home/me/.lope/config.json");
        } else {
            panic!("expected Adapter::MigrateConfig");
        }
    }

    #[test]
    fn parse_adapter_export() {
        let cli = Cli::try_parse_from([
            "makakoo", "adapter", "export", "openclaw", "--out", "/tmp/openclaw.tgz",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Export { name, out },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert_eq!(out.unwrap().to_str().unwrap(), "/tmp/openclaw.tgz");
        } else {
            panic!("expected Adapter::Export");
        }
    }

    #[test]
    fn parse_adapter_call_with_prompt() {
        let cli = Cli::try_parse_from([
            "makakoo",
            "adapter",
            "call",
            "openclaw",
            "--prompt",
            "hello",
            "--timeout",
            "120",
            "--bundled",
        ])
        .unwrap();
        if let Commands::Adapter {
            cmd:
                AdapterCmd::Call {
                    name,
                    prompt,
                    timeout,
                    bundled,
                },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert_eq!(prompt.as_deref(), Some("hello"));
            assert_eq!(timeout, 120);
            assert!(bundled);
        } else {
            panic!("expected Adapter::Call");
        }
    }

    #[test]
    fn parse_adapter_info_json() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "info", "openclaw", "--json"])
            .unwrap();
        if let Commands::Adapter {
            cmd: AdapterCmd::Info { name, json },
        } = cli.command.unwrap()
        {
            assert_eq!(name, "openclaw");
            assert!(json);
        } else {
            panic!("expected Adapter::Info");
        }
    }

    #[test]
    fn parse_adapter_spec() {
        let cli = Cli::try_parse_from(["makakoo", "adapter", "spec"]).unwrap();
        matches!(
            cli.command.unwrap(),
            Commands::Adapter {
                cmd: AdapterCmd::Spec,
            }
        );
    }
}
