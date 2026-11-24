//! `makakoo sancho tick|status` — SANCHO proactive task engine.
//!
//! `tick` constructs the default registry and fires every eligible
//! handler exactly once. `status` prints registered tasks and their
//! last-run timestamps (best-effort introspection — we query gate
//! state directly since there's no persisted task ledger).

use std::sync::Arc;
use std::time::Duration;

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};

use makakoo_core::plugin::PluginRegistry;
use makakoo_core::sancho::{default_registry, SanchoContext, SanchoEngine, NATIVE_TASK_COUNT};

use crate::cli::SanchoCmd;
use crate::context::CliContext;
use crate::output;

pub async fn run(ctx: &CliContext, cmd: SanchoCmd) -> anyhow::Result<i32> {
    match cmd {
        SanchoCmd::Tick => tick(ctx).await,
        SanchoCmd::Status => status(ctx).await,
    }
}

async fn build_engine(ctx: &CliContext) -> anyhow::Result<SanchoEngine> {
    let store = ctx.store()?;
    let bus = ctx.event_bus()?;
    let llm = ctx.llm();
    let emb = ctx.embeddings();
    let bulk_emb = ctx.bulk_embeddings();
    let sancho_ctx = Arc::new(
        SanchoContext::new(store, bus, llm, emb, ctx.home().clone()).with_bulk_embeddings(bulk_emb),
    );
    let plugins = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    let registry = default_registry(Arc::clone(&sancho_ctx), &plugins);
    Ok(SanchoEngine::new(
        registry,
        sancho_ctx,
        Duration::from_secs(60),
    ))
}

async fn tick(ctx: &CliContext) -> anyhow::Result<i32> {
    Ok(run_tick(ctx).await?.0)
}

async fn run_tick(ctx: &CliContext) -> anyhow::Result<(i32, bool)> {
    let engine = build_engine(ctx).await?;
    output::print_info(format!(
        "sancho: ticking {} registered task(s)…",
        engine.task_count()
    ));
    let reports = engine.tick_once().await?;
    output::print_handler_reports(&reports);
    let rebuild_ran = engine.context().brain_rebuild.spawned_count() > 0;
    if rebuild_ran {
        engine.context().brain_rebuild.wait_all().await;
        output::print_info("sancho: brain vector rebuild finished");
    }
    let failed = reports.iter().any(|r| !r.ok);
    Ok((if failed { 2 } else { 0 }, rebuild_ran))
}

async fn status(ctx: &CliContext) -> anyhow::Result<i32> {
    let engine = build_engine(ctx).await?;
    let state = engine.state();
    let guard = state.lock().await;
    let total = engine.task_count();
    let manifest = total.saturating_sub(NATIVE_TASK_COUNT);
    output::print_info(format!(
        "sancho: {total} registered task(s) ({native} native + {manifest} manifest)",
        native = NATIVE_TASK_COUNT,
    ));
    if guard.last_run.is_empty() {
        output::print_info("(no task has run yet this process)");
    } else {
        let mut t = Table::new();
        t.load_preset(UTF8_FULL);
        t.set_header(vec![
            Cell::new("task").fg(TableColor::Cyan),
            Cell::new("last_run").fg(TableColor::Cyan),
            Cell::new("busy").fg(TableColor::Cyan),
        ]);
        for (task, ts) in guard.last_run.iter() {
            let busy = guard.locks.get(task).copied().unwrap_or(false);
            t.add_row(vec![
                Cell::new(task),
                Cell::new(ts.to_rfc3339()),
                Cell::new(busy.to_string()),
            ]);
        }
        println!("{t}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use makakoo_core::superbrain::folder_index::IndexModelState;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Notify;

    use super::*;

    struct EnvRestore(Vec<(&'static str, Option<String>)>);

    impl EnvRestore {
        fn set(pairs: &[(&'static str, &str)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(k, _)| (*k, std::env::var(k).ok()))
                .collect();
            for (k, v) in pairs {
                std::env::set_var(k, v);
            }
            Self(saved)
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    async fn read_http_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break buf.len();
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let header_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_len = header_str
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        while buf.len() < header_end + content_len {
            let n = stream.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let first_line = header_str.lines().next().unwrap_or("").to_string();
        let body = buf.get(header_end..).unwrap_or_default().to_vec();
        (first_line, body)
    }

    async fn spawn_gated_sancho_mock(
        gate_entered: Arc<AtomicBool>,
        gate_release: Arc<Notify>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let entered = Arc::clone(&gate_entered);
                let release = Arc::clone(&gate_release);
                tokio::spawn(async move {
                    let (req_line, body_bytes) = read_http_request(&mut stream).await;
                    let resp_body = if req_line.contains("/embeddings") {
                        let body: Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
                        let inputs = body
                            .get("input")
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or(1);
                        let is_probe = inputs == 1
                            && body
                                .get("input")
                                .and_then(|v| v.get(0))
                                .and_then(Value::as_str)
                                == Some("probe");
                        if !is_probe {
                            let notified = release.notified();
                            tokio::pin!(notified);
                            notified.as_mut().enable();
                            entered.store(true, Ordering::SeqCst);
                            notified.await;
                        }
                        let data: Vec<Value> = (0..inputs)
                            .map(|_| json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                            .collect();
                        json!({ "model": "model-v2", "data": data }).to_string()
                    } else {
                        json!({
                            "choices": [{ "message": { "content": "NOOP" } }]
                        })
                        .to_string()
                    };
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn one_shot_sancho_tick_waits_for_scheduled_rebuild_to_finish() {
        let _guard = crate::test_support::ENV_MUTEX.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data/Brain/journals")).unwrap();
        let ctx = CliContext::for_home(dir.path().to_path_buf());
        let store = ctx.store().unwrap();
        store
            .write_document(
                "pages/note.md",
                "Pod one-shot sancho tick rebuild wait test.",
                "page",
                json!([]),
            )
            .unwrap();
        store.mark_vectors_stale("model_changed").unwrap();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let gate_entered = Arc::new(AtomicBool::new(false));
        let gate_release = Arc::new(Notify::new());
        let base_url = rt.block_on(spawn_gated_sancho_mock(
            Arc::clone(&gate_entered),
            Arc::clone(&gate_release),
        ));
        let _env = EnvRestore::set(&[
            ("AIL_BASE_URL", &base_url),
            ("MAKAKOO_EMBED_MODEL", "short-alias"),
            ("MAKAKOO_BULK_EMBED_MODEL", "bulk-alias"),
            ("MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC", "1"),
        ]);

        rt.block_on(async move {
            let store_check = Arc::clone(&store);
            let tick_handle = tokio::spawn(async move { run_tick(&ctx).await });

            tokio::time::timeout(Duration::from_secs(10), async {
                while !gate_entered.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("scheduled rebuild never reached batch embed");

            // While the rebuild is in flight, one-shot `tick` must still be waiting.
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                !tick_handle.is_finished(),
                "one-shot sancho tick exited before its scheduled rebuild finished"
            );
            assert_eq!(
                store_check.vector_status().unwrap().index_state,
                IndexModelState::Rebuilding
            );

            gate_release.notify_waiters();
            let (code, rebuild_ran) = tokio::time::timeout(Duration::from_secs(10), tick_handle)
                .await
                .expect("one-shot sancho tick did not finish after gate release")
                .unwrap()
                .unwrap();
            assert_eq!(code, 0);
            assert!(rebuild_ran);

            let status = store_check.vector_status().unwrap();
            assert_eq!(status.index_state, IndexModelState::Ready);
            assert_eq!(status.embed_model.as_deref(), Some("model-v2"));
            assert_eq!(status.embed_dim, Some(4));
            assert_eq!(store_check.stats().unwrap().vector_count, 1);
        });
    }
}
