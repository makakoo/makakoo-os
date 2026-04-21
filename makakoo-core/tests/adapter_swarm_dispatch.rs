//! Swarm × adapter integration — proves that
//! `SwarmGateway::dispatch(DispatchRequest { adapter: Some("openclaw"), .. })`
//! routes through the universal-bridge pipeline, writes Plan + Result
//! artifacts in the standard shape, and surfaces adapter INFRA_ERROR
//! without crashing the dispatch pipeline.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use makakoo_core::db::{open_db, run_migrations};

/// Env mutation is process-global. Serialize these tests so each one
/// sees a clean MAKAKOO_HOME / MAKAKOO_ADAPTERS_HOME pair.
fn env_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::swarm::{
    AgentCoordinator, ArtifactKind, ArtifactStore, DispatchRequest, SwarmGateway,
};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a gateway under an isolated $MAKAKOO_HOME and return the Arc
/// handles so tests can still query the coordinator + artifact store
/// independently of the gateway (whose fields are private).
struct Fixture {
    _home: tempfile::TempDir,
    gateway: SwarmGateway,
    coordinator: Arc<AgentCoordinator>,
    artifacts: Arc<ArtifactStore>,
}

async fn build_fixture(llm_base: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("MAKAKOO_HOME", home.path());
    std::env::set_var(
        "MAKAKOO_ADAPTERS_HOME",
        home.path().join(".makakoo/adapters"),
    );
    let conn = open_db(&home.path().join("smoke.db")).unwrap();
    run_migrations(&conn).unwrap();
    let conn_arc = Arc::new(Mutex::new(conn));
    let artifacts = Arc::new(ArtifactStore::open(Arc::clone(&conn_arc)).unwrap());
    let coordinator = Arc::new(AgentCoordinator::new());
    let llm = Arc::new(LlmClient::with_base_url(llm_base.to_string()));
    let bus = PersistentEventBus::open(&home.path().join("bus.db")).unwrap();
    let gateway = SwarmGateway::new(
        Arc::clone(&coordinator),
        Arc::clone(&artifacts),
        llm,
        bus,
    );
    Fixture {
        _home: home,
        gateway,
        coordinator,
        artifacts,
    }
}

fn install_adapter(adapters_home: &std::path::Path, base_url: &str) {
    let reg = adapters_home.join("registered");
    std::fs::create_dir_all(&reg).unwrap();
    let body = format!(
        r#"
[adapter]
name = "openclaw"
version = "0.1.0"
manifest_schema = 1
description = "test openclaw adapter"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "{base_url}"

[auth]
scheme = "none"

[output]
format = "openai-chat"
verdict_field = "choices.0.message.content"

[capabilities]
supports_roles = ["validator", "delegate", "swarm_member"]

[install]
source_type = "local"

[security]
requires_network = true
allowed_hosts = ["127.0.0.1"]
sandbox_profile = "network-io"
"#,
    );
    std::fs::write(reg.join("openclaw.toml"), body).unwrap();
}

#[tokio::test]
async fn swarm_dispatch_with_adapter_writes_result_artifact() {
    let _g = env_guard();
    let mock = MockServer::start().await;
    let adapter_url = format!("{}/v1", mock.uri());
    Mock::given(method("POST"))
        .and(wm_path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [
                {"message": {"content": "---VERDICT---\nstatus: PASS\nconfidence: 0.9\nrationale: adapter-said-ok\n---END---"}}
            ]
        })))
        .mount(&mock)
        .await;

    let fx = build_fixture("http://127.0.0.1:1/v1").await;
    install_adapter(
        &std::env::var("MAKAKOO_ADAPTERS_HOME")
            .map(std::path::PathBuf::from)
            .unwrap(),
        &adapter_url,
    );

    let resp = fx
        .gateway
        .dispatch(DispatchRequest {
            name: "openclaw-sub".into(),
            task: "delegate to openclaw".into(),
            prompt: "tell me the VERDICT".into(),
            model: None,
            parent_run_id: None,
            adapter: Some("openclaw".into()),
        })
        .await
        .unwrap();
    assert!(resp.accepted);
    let _ = fx.coordinator.wait(&resp.subagent_id).await.unwrap();

    let arts = fx.artifacts.by_run(&resp.run_id).unwrap();
    assert!(
        arts.iter().any(|a| a.kind == ArtifactKind::Plan),
        "missing Plan artifact"
    );
    let result_art = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::Result)
        .expect("result artifact");
    assert!(
        result_art.content.contains("adapter-said-ok"),
        "got {:?}",
        result_art.content
    );
    assert_eq!(
        result_art.metadata["adapter"].as_str(),
        Some("openclaw")
    );
    assert_eq!(
        result_art.metadata["status"].as_str(),
        Some("PASS")
    );
}

#[tokio::test]
async fn swarm_dispatch_with_unregistered_adapter_fails_cleanly() {
    let _g = env_guard();
    let fx = build_fixture("http://127.0.0.1:1/v1").await;
    // Deliberately don't install anything — registry is empty.

    let resp = fx
        .gateway
        .dispatch(DispatchRequest {
            name: "ghost".into(),
            task: "go".into(),
            prompt: "anything".into(),
            model: None,
            parent_run_id: None,
            adapter: Some("does-not-exist".into()),
        })
        .await
        .unwrap();
    let outcome = fx.coordinator.wait(&resp.subagent_id).await;
    assert!(
        outcome.is_err(),
        "dispatch with bogus adapter must surface err"
    );

    let arts = fx.artifacts.by_run(&resp.run_id).unwrap();
    assert!(
        !arts.iter().any(|a| a.kind == ArtifactKind::Result),
        "no result artifact should land for an unregistered adapter"
    );
}

#[tokio::test]
async fn swarm_dispatch_adapter_infra_error_still_writes_artifact() {
    let _g = env_guard();
    let fx = build_fixture("http://127.0.0.1:1/v1").await;
    install_adapter(
        &std::env::var("MAKAKOO_ADAPTERS_HOME")
            .map(std::path::PathBuf::from)
            .unwrap(),
        "http://127.0.0.1:2",
    );

    let resp = fx
        .gateway
        .dispatch(DispatchRequest {
            name: "openclaw-sub".into(),
            task: "fail-probe".into(),
            prompt: "this will timeout".into(),
            model: None,
            parent_run_id: None,
            adapter: Some("openclaw".into()),
        })
        .await
        .unwrap();
    let _ = fx.coordinator.wait(&resp.subagent_id).await.unwrap();

    let arts = fx.artifacts.by_run(&resp.run_id).unwrap();
    let result_art = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::Result)
        .expect("result artifact for infra error");
    assert_eq!(
        result_art.metadata["status"].as_str(),
        Some("INFRA_ERROR"),
        "adapter timeout should surface as INFRA_ERROR in metadata"
    );
}

#[tokio::test]
async fn dispatch_request_roundtrips_adapter_field_via_json() {
    let req = DispatchRequest {
        name: "x".into(),
        task: "t".into(),
        prompt: "p".into(),
        model: None,
        parent_run_id: None,
        adapter: Some("openclaw".into()),
    };
    let wire = serde_json::to_string(&req).unwrap();
    assert!(wire.contains("\"adapter\":\"openclaw\""));
    let round: DispatchRequest = serde_json::from_str(&wire).unwrap();
    assert_eq!(round.adapter.as_deref(), Some("openclaw"));
}
