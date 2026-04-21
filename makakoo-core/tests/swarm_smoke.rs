//! Wave 4 T15 swarm smoke test — spawn 3 fake subagents, wait for all,
//! verify artifacts landed. Not a unit test — an end-to-end harness that
//! exercises the full coordinator + artifact store + gateway path with a
//! stub LLM wiremock server.

use std::sync::{Arc, Mutex};

use makakoo_core::db::{open_db, run_migrations};
use makakoo_core::event_bus::PersistentEventBus;
use makakoo_core::llm::LlmClient;
use makakoo_core::swarm::{
    AgentCoordinator, ArtifactKind, ArtifactStore, DispatchRequest, SwarmGateway,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn smoke_three_subagents_complete_and_write_artifacts() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "smoke-ok"}}]
        })))
        .mount(&mock)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("smoke.db");
    let conn = open_db(&db_path).unwrap();
    run_migrations(&conn).unwrap();
    let conn_arc = Arc::new(Mutex::new(conn));
    let artifacts = Arc::new(ArtifactStore::open(Arc::clone(&conn_arc)).unwrap());
    let coordinator = Arc::new(AgentCoordinator::new());
    let llm = Arc::new(LlmClient::with_base_url(format!("{}/v1", mock.uri())));
    let bus = PersistentEventBus::open(&dir.path().join("bus.db")).unwrap();
    let gateway = SwarmGateway::new(
        Arc::clone(&coordinator),
        Arc::clone(&artifacts),
        llm,
        bus,
    );

    // Dispatch 3 fake subagents.
    let mut responses = Vec::new();
    for i in 0..3 {
        let resp = gateway
            .dispatch(DispatchRequest {
                name: format!("fake-{i}"),
                task: format!("task-{i}"),
                prompt: format!("do thing {i}"),
                model: None,
                parent_run_id: None,
                adapter: None,
            })
            .await
            .expect("dispatch accepted");
        responses.push(resp);
    }
    assert_eq!(responses.len(), 3);

    // Wait for every subagent to finish.
    for r in &responses {
        let out = coordinator
            .wait(&r.subagent_id)
            .await
            .expect("subagent completed");
        assert_eq!(out["content"].as_str().unwrap(), "smoke-ok");
    }

    // Every run should now have at least a Plan + Result artifact.
    for r in &responses {
        let arts = artifacts.by_run(&r.run_id).unwrap();
        assert!(arts.iter().any(|a| a.kind == ArtifactKind::Plan));
        assert!(arts.iter().any(|a| a.kind == ArtifactKind::Result));
    }
    // And `by_kind` should show at least 3 result artifacts overall.
    let results = artifacts.by_kind(ArtifactKind::Result, 10).unwrap();
    assert!(
        results.len() >= 3,
        "expected ≥3 Result artifacts, got {}",
        results.len()
    );
}
