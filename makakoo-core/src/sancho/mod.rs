//! SANCHO — proactive task scheduler.
//!
//! Rust port of the Python `core/sancho/` package. SANCHO runs background
//! maintenance tasks (memory consolidation, wiki lint, superbrain sync,
//! daily briefings, etc.) on a gated tick loop. Each task is a
//! [`SanchoHandler`] paired with a set of [`Gate`]s that decide whether
//! the handler may run at the current moment.
//!
//! Phase C (2026-04-15): subprocess tasks (watchdogs, GYM) no longer live
//! as hardcoded registrations here. They are discovered from plugin
//! manifests under `$MAKAKOO_HOME/plugins/*/plugin.toml` via
//! [`register_plugin_sancho_tasks`]. The 8 pure-Rust handlers below are
//! the only hardcoded tasks left — they belong to the kernel, not to any
//! plugin.

pub mod engine;
pub mod gates;
pub mod handlers;
pub mod registry;

pub use engine::SanchoEngine;
pub use gates::{
    ActiveHoursGate, Gate, GateState, LockGate, SessionGate, TimeGate, WeekdayGate,
};
pub use handlers::{
    DailyBriefingHandler, DreamHandler, DynamicChecklistHandler, FakeLlmCall,
    IndexRebuildHandler, LlmCall, MemoryConsolidationHandler, MemoryPromotionHandler,
    SubprocessHandler, SuperbrainSyncEmbedHandler, SwarmDispatchHandler, WikiLintHandler,
};
pub use registry::{HandlerReport, SanchoContext, SanchoHandler, SanchoRegistry, TaskRegistration};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::plugin::PluginRegistry;

/// Parse a human-readable duration like `"5m"`, `"300s"`, `"24h"`, `"7d"`.
/// Falls back to `default` on any parse failure so plugin manifests
/// cannot crash the registry build — a bad interval becomes a logged
/// warning in the caller.
pub fn parse_interval(spec: &str, default: Duration) -> Duration {
    let s = spec.trim();
    if s.is_empty() {
        return default;
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let last = s.chars().last().unwrap_or(' ');
    let (num_str, mul) = match last {
        's' => (num_part, 1u64),
        'm' => (num_part, 60),
        'h' => (num_part, 3600),
        'd' => (num_part, 86400),
        c if c.is_ascii_digit() => (s, 1),
        _ => return default,
    };
    let _ = unit; // silence unused
    match num_str.parse::<u64>() {
        Ok(n) => Duration::from_secs(n.saturating_mul(mul)),
        Err(_) => default,
    }
}

/// Number of pure-Rust handlers the kernel registers before any plugin
/// is walked. Bumped only when a new native handler ships with the
/// kernel itself (not a plugin). Locked by `native_registry_has_exactly_eight`.
///
/// Used by `makakoo sancho status` to display a "N native + M manifest"
/// breakdown without rebuilding the native registry twice.
pub const NATIVE_TASK_COUNT: usize = 9;

/// The eight native SANCHO task names the kernel owns. A plugin whose
/// manifest declares `[[sancho.tasks]].name` matching any of these
/// shadows the kernel handler, which would either double-register (two
/// handlers firing for the same name) or silently replace the native
/// implementation with a subprocess.
///
/// Both paths are bugs. `install_from_path` rejects plugins that would
/// collide before they ever land on disk; `register_plugin_sancho_tasks`
/// skips them defensively if a manifest somehow arrives by bypassing
/// `plugin install` (e.g. hand-copied into `$MAKAKOO_HOME/plugins/`).
///
/// Locked by `native_task_names_match_registry` test — adding a 9th
/// native handler without updating this list fails the build.
pub const NATIVE_TASK_NAMES: &[&str] = &[
    "dream",
    "wiki_lint",
    "index_rebuild",
    "daily_briefing",
    "memory_consolidation",
    "memory_promotion",
    "superbrain_sync_embed",
    "dynamic_checklist",
    "swarm_dispatch",
];

/// Build the kernel's native SANCHO registry — 8 pure-Rust handlers that
/// ship with the kernel and never come from a plugin.
pub fn native_registry(ctx: Arc<SanchoContext>) -> SanchoRegistry {
    let mut reg = SanchoRegistry::new();
    let llm_for_dream: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;
    let llm_for_brief: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;
    let llm_for_check: Arc<dyn LlmCall> = Arc::clone(&ctx.llm) as Arc<dyn LlmCall>;

    reg.register(
        Arc::new(DreamHandler::new(llm_for_dream)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(4 * 3600))),
            Arc::new(SessionGate),
            Arc::new(LockGate),
        ],
    );
    reg.register(
        Arc::new(WikiLintHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(6 * 3600)))],
    );
    reg.register(
        Arc::new(IndexRebuildHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(12 * 3600)))],
    );
    reg.register(
        Arc::new(DailyBriefingHandler::new(llm_for_brief)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(8 * 3600))),
            Arc::new(ActiveHoursGate::new(7, 22)),
        ],
    );
    reg.register(
        Arc::new(MemoryConsolidationHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(4 * 3600)))],
    );
    reg.register(
        Arc::new(MemoryPromotionHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(20 * 3600)))],
    );
    reg.register(
        Arc::new(SuperbrainSyncEmbedHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(12 * 60)))],
    );
    reg.register(
        Arc::new(DynamicChecklistHandler::new(llm_for_check)),
        vec![
            Arc::new(TimeGate::new(Duration::from_secs(3600))),
            Arc::new(ActiveHoursGate::new(8, 22)),
        ],
    );
    // v0.2 D.4/C.6 — swarm dispatch queue drainer. Runs every 60s so
    // producers enqueueing work don't wait for a long cadence. The
    // handler no-ops if the queue is empty, so the cadence is cheap.
    reg.register(
        Arc::new(SwarmDispatchHandler::new()),
        vec![Arc::new(TimeGate::new(Duration::from_secs(60)))],
    );
    reg
}

/// Walk a [`PluginRegistry`] and add a [`SubprocessHandler`] to `reg` for
/// every `[sancho].tasks` entry declared by any plugin. The plugin's
/// `[entrypoint].run` command is the base invocation; the task name is
/// appended as `--task <name>` (matching the convention used by the
/// hardcoded GYM dispatcher the Python version shipped with).
///
/// Plugins whose `run` is missing are silently skipped — the manifest
/// parser already rejects SanchoTask-kind plugins without `run`, so this
/// branch only fires for Agent-kind plugins that declare sancho tasks as
/// a side channel without providing `run` (which is a manifest-level
/// modeling mistake we tolerate gracefully).
pub fn register_plugin_sancho_tasks(reg: &mut SanchoRegistry, plugins: &PluginRegistry) {
    for plugin in plugins.plugins() {
        // Soft toggle: plugins disabled via `makakoo plugin disable`
        // are still discovered (so `plugin list` and `plugin info` can
        // surface them) but skip task registration. Re-enable restores
        // them on the next registry load without reinstalling.
        if !plugin.enabled {
            continue;
        }
        let Some(run_cmd) = plugin.manifest.entrypoint.run.clone() else {
            continue;
        };
        for task in &plugin.manifest.sancho.tasks {
            // Defensive collision check — `install_from_path` already
            // rejects plugins whose task names shadow native handlers.
            // This branch only fires if a manifest arrived by bypassing
            // the installer (hand-copied into $MAKAKOO_HOME/plugins/).
            // Skipping preserves the native handler; the warning makes
            // the situation visible in logs without crashing the boot.
            if NATIVE_TASK_NAMES.iter().any(|n| *n == task.name.as_str()) {
                tracing::warn!(
                    plugin = %plugin.manifest.plugin.name,
                    task = %task.name,
                    "skipping plugin task that collides with native SANCHO handler — \
                     native implementation wins. Rename the plugin task or run \
                     `makakoo plugin uninstall {}` to clear the conflict.",
                    plugin.manifest.plugin.name,
                );
                continue;
            }
            let handler = build_subprocess_handler(&plugin.root, &run_cmd, &task.name);
            let mut gates: Vec<Arc<dyn Gate>> = Vec::new();
            gates.push(Arc::new(TimeGate::new(parse_interval(
                &task.interval,
                Duration::from_secs(3600),
            ))));
            if let Some([start, end]) = task.active_hours {
                gates.push(Arc::new(ActiveHoursGate::new(
                    u32::from(start),
                    u32::from(end),
                )));
            }
            for gate_name in &task.gates {
                match gate_name.as_str() {
                    "session" => gates.push(Arc::new(SessionGate)),
                    "lock" => gates.push(Arc::new(LockGate)),
                    _ => {}
                }
            }
            reg.register(Arc::new(handler), gates);
        }
    }
}

/// Build a [`SubprocessHandler`] for a plugin sancho task. Splits the
/// plugin's `[entrypoint].run` string on whitespace so we can re-inject
/// `--task <name>` as extra args. Paths in the entrypoint are resolved
/// relative to the plugin root:
///
/// - Program path is rewritten when it starts with `./` or `.venv`.
/// - The subprocess CWD is set to `plugin_root` so any relative arg
///   like `src/run.py` resolves inside the plugin's own bundled
///   source tree (the v0.1 self-contained layout). `$MAKAKOO_HOME`
///   stays exported in env so plugins can still reach shared state.
fn build_subprocess_handler(
    plugin_root: &Path,
    run_cmd: &str,
    task_name: &str,
) -> SubprocessHandler {
    let mut parts = run_cmd.split_whitespace();
    let program = parts.next().unwrap_or("").to_string();
    let program_path = if program.starts_with("./") || program.starts_with(".venv") {
        plugin_root.join(&program).to_string_lossy().into_owned()
    } else {
        program
    };
    let mut args: Vec<String> = parts.map(|s| s.to_string()).collect();
    args.push("--task".to_string());
    args.push(task_name.to_string());
    SubprocessHandler::new(task_name, program_path, args).with_cwd(plugin_root)
}

/// Build the default production registry. Pass `plugins` to inject
/// manifest-driven subprocess tasks; pass `&PluginRegistry::default()` to
/// get just the 8 native Rust handlers (used by tests and for the fresh
/// install smoke).
pub fn default_registry(
    ctx: Arc<SanchoContext>,
    plugins: &PluginRegistry,
) -> SanchoRegistry {
    let mut reg = native_registry(ctx);
    register_plugin_sancho_tasks(&mut reg, plugins);
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::EmbeddingClient;
    use crate::event_bus::PersistentEventBus;
    use crate::llm::LlmClient;
    use crate::superbrain::store::SuperbrainStore;
    use tempfile::TempDir;

    fn make_ctx(home: &std::path::Path) -> Arc<SanchoContext> {
        let store = Arc::new(SuperbrainStore::open(&home.join("b.db")).unwrap());
        let bus = PersistentEventBus::open(&home.join("bus.db")).unwrap();
        let llm = Arc::new(LlmClient::new());
        let emb = Arc::new(EmbeddingClient::new());
        Arc::new(SanchoContext::new(store, bus, llm, emb, home.to_path_buf()))
    }

    fn seed_plugin(home: &std::path::Path, dir_name: &str, body: &str) {
        let p = home.join("plugins").join(dir_name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("plugin.toml"), body).unwrap();
    }

    #[test]
    fn fresh_install_registers_only_native_tasks() {
        // No plugins/ dir, no manifests. The kernel's 8 native Rust
        // handlers are the entire surface.
        let dir = TempDir::new().unwrap();
        let plugins = PluginRegistry::load_default(dir.path()).unwrap();
        let reg = default_registry(make_ctx(dir.path()), &plugins);
        assert_eq!(
            reg.len(),
            NATIVE_TASK_COUNT,
            "fresh install with no plugins should yield exactly NATIVE_TASK_COUNT native tasks"
        );
    }

    #[test]
    fn native_registry_count_matches_const() {
        let dir = TempDir::new().unwrap();
        let reg = native_registry(make_ctx(dir.path()));
        assert_eq!(reg.len(), NATIVE_TASK_COUNT);
    }

    #[test]
    fn native_task_count_constant_matches_registry() {
        // The `sancho status` command displays a native-vs-manifest
        // breakdown by subtracting NATIVE_TASK_COUNT from the total. If
        // someone adds a 9th native handler without bumping the constant,
        // the breakdown silently becomes wrong. This test keeps them locked.
        let dir = TempDir::new().unwrap();
        let reg = native_registry(make_ctx(dir.path()));
        assert_eq!(
            reg.len(),
            NATIVE_TASK_COUNT,
            "NATIVE_TASK_COUNT must equal native_registry().len()"
        );
    }

    #[test]
    fn plugin_with_one_sancho_task_registers() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_plugin(
            home,
            "watchdog-postgres",
            r#"
[plugin]
name = "watchdog-postgres"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -m pg_watchdog"

[sancho]
tasks = [{ name = "pg_watchdog", interval = "900s" }]
"#,
        );
        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        assert_eq!(reg.len(), NATIVE_TASK_COUNT + 1, "native + 1 plugin task");
    }

    #[test]
    fn plugin_with_multiple_sancho_tasks_all_register() {
        // Five tasks under one plugin — mirrors the GYM shape that used
        // to live as hardcoded SanchoSubprocess::gym calls in this file.
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_plugin(
            home,
            "mascot-gym",
            r#"
[plugin]
name = "mascot-gym"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -m gym.run"

[sancho]
tasks = [
  { name = "gym_classify", interval = "3600s" },
  { name = "gym_hypothesize", interval = "84600s", active_hours = [1, 4] },
  { name = "gym_lope_gate", interval = "84600s", active_hours = [3, 6] },
  { name = "gym_morning_report", interval = "84600s", active_hours = [6, 9] },
  { name = "gym_weekly_report", interval = "604800s", active_hours = [8, 11] },
]
"#,
        );
        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        assert_eq!(reg.len(), NATIVE_TASK_COUNT + 5, "native + 5 gym tasks");
    }

    #[test]
    fn watchdog_infect_plugin_registers() {
        // Sprint-008 plugin. Test asserts registry *contains* a handler
        // named `watchdog_infect` rather than asserting a fragile total
        // (len() == N breaks whenever any other native task lands).
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_plugin(
            home,
            "watchdog-infect",
            r#"
[plugin]
name = "watchdog-infect"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -u plugins/watchdog-infect/watchdog.py"

[sancho]
tasks = [{ name = "watchdog_infect", interval = "21600s" }]
"#,
        );
        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        let names: Vec<&str> = reg.tasks().iter().map(|t| t.handler.name()).collect();
        assert!(
            names.contains(&"watchdog_infect"),
            "expected watchdog_infect in registered tasks; got {names:?}"
        );
        assert!(
            reg.len() >= NATIVE_TASK_COUNT + 1,
            "native + at least 1 plugin task expected, got {}",
            reg.len()
        );
    }

    #[test]
    fn walker_skips_plugin_task_that_shadows_native_handler() {
        // Belt-and-suspenders defense: `install_from_path` rejects
        // collision-bearing plugins before they land on disk, but if
        // a manifest arrives by hand-copy into $MAKAKOO_HOME/plugins/,
        // the walker still refuses to register the shadowing task and
        // logs a warning. The native handler keeps running; the plugin
        // just loses its shadowed entry.
        let dir = TempDir::new().unwrap();
        let home = dir.path();

        seed_plugin(
            home,
            "naughty-plugin",
            r#"
[plugin]
name = "naughty-plugin"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -m naughty"

[sancho]
tasks = [{ name = "dream", interval = "3600s" }]
"#,
        );
        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        let names: Vec<&str> = reg.tasks().iter().map(|t| t.handler.name()).collect();
        let dream_count = names.iter().filter(|n| **n == "dream").count();
        assert_eq!(
            dream_count, 1,
            "walker must skip plugin tasks that shadow native handler names"
        );
        // Registry total: NATIVE_TASK_COUNT + 0 plugin-derived (shadowed task dropped).
        assert_eq!(
            reg.len(),
            NATIVE_TASK_COUNT,
            "naughty plugin's single task is shadowed, so no plugin tasks register"
        );
    }

    #[test]
    fn native_task_names_match_registry() {
        // The NATIVE_TASK_NAMES constant and the actual native_registry()
        // drift silently if a new handler ships without touching the list.
        // This test reads every handler's reported name and compares it
        // element-wise to the constant. Ordering matters — bump the const
        // in the same order when adding a handler.
        let dir = TempDir::new().unwrap();
        let reg = native_registry(make_ctx(dir.path()));
        let actual: Vec<&str> = reg.tasks().iter().map(|t| t.handler.name()).collect();
        assert_eq!(actual.len(), NATIVE_TASK_NAMES.len());
        for (got, expected) in actual.iter().zip(NATIVE_TASK_NAMES.iter()) {
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn disabled_plugin_does_not_register_sancho_tasks() {
        // When a plugin is soft-disabled via `makakoo plugin disable`, its
        // plugins.lock entry carries enabled = false. `PluginRegistry::load_default`
        // overlays that flag; `register_plugin_sancho_tasks` skips such plugins.
        // Re-enabling restores registration on the next load.
        use crate::plugin::{lock_path, LockEntry, PluginsLock};
        use chrono::Utc;

        let dir = TempDir::new().unwrap();
        let home = dir.path();
        seed_plugin(
            home,
            "togglable",
            r#"
[plugin]
name = "togglable"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^1.0"

[entrypoint]
run = "python3 -m togglable"

[sancho]
tasks = [{ name = "togglable_tick", interval = "3600s" }]
"#,
        );

        // Fresh: no lock file, plugin defaults to enabled, task registers.
        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        assert_eq!(reg.len(), NATIVE_TASK_COUNT + 1, "fresh install — native + 1 plugin task");

        // Disable via lock file: task must drop out.
        let mut lock = PluginsLock::default();
        lock.upsert(LockEntry {
            name: "togglable".into(),
            version: "1.0.0".into(),
            blake3: None,
            source: "test".into(),
            resolved_sha: None,
            manifest_hash: None,
            installed_at: Utc::now(),
            enabled: false,
        });
        lock.save(home).unwrap();
        assert!(lock_path(home).exists());

        let plugins = PluginRegistry::load_default(home).unwrap();
        assert!(
            !plugins.get("togglable").unwrap().enabled,
            "registry must reflect lock's enabled=false"
        );
        let reg = default_registry(make_ctx(home), &plugins);
        assert_eq!(reg.len(), NATIVE_TASK_COUNT, "disabled plugin must not register");

        // Re-enable: task comes back without reinstalling.
        let mut lock = PluginsLock::load(home).unwrap();
        let mut entry = lock.get("togglable").unwrap().clone();
        entry.enabled = true;
        lock.upsert(entry);
        lock.save(home).unwrap();

        let plugins = PluginRegistry::load_default(home).unwrap();
        let reg = default_registry(make_ctx(home), &plugins);
        assert_eq!(reg.len(), NATIVE_TASK_COUNT + 1, "re-enable restores the task");
    }

    #[test]
    fn parse_interval_handles_common_shapes() {
        assert_eq!(
            parse_interval("5m", Duration::from_secs(1)),
            Duration::from_secs(300)
        );
        assert_eq!(
            parse_interval("300s", Duration::from_secs(1)),
            Duration::from_secs(300)
        );
        assert_eq!(
            parse_interval("24h", Duration::from_secs(1)),
            Duration::from_secs(86400)
        );
        assert_eq!(
            parse_interval("7d", Duration::from_secs(1)),
            Duration::from_secs(604800)
        );
        // Garbage falls back to default.
        assert_eq!(
            parse_interval("garbage", Duration::from_secs(42)),
            Duration::from_secs(42)
        );
    }
}
