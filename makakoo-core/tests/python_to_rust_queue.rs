//! D.4 cross-language contract test.
//!
//! Python writes a queue line via `core.orchestration.swarm_dispatch_queue`.
//! Rust reads it back via `swarm::dispatch_queue::load_queue`. The shapes
//! MUST agree — this is the only runtime contract between the two halves.

use std::process::Command;

use makakoo_core::swarm::{load_queue, QueueEntry};

/// Skip unless python3 is on PATH and the lib-harvey-core tree is
/// present relative to the test's invocation dir.
fn python_available() -> Option<std::path::PathBuf> {
    let py = Command::new("python3").arg("--version").output().ok()?;
    if !py.status.success() {
        return None;
    }
    // `cargo test` runs with cwd = crate root (makakoo-core/). The
    // lib-harvey-core tree lives one level up.
    let core_py = std::path::PathBuf::from("../plugins-core/lib-harvey-core/src");
    if core_py.is_dir() {
        Some(core_py)
    } else {
        None
    }
}

#[test]
fn rust_reads_queue_entries_python_wrote() {
    let Some(py_path) = python_available() else {
        eprintln!("skipping: python3 or plugins-core/lib-harvey-core not reachable");
        return;
    };
    let dir = tempfile::tempdir().unwrap();

    // Invoke the Python shim to enqueue two entries.
    let status = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import os, sys
sys.path.insert(0, os.environ["PY_PATH"])
from core.orchestration.swarm_dispatch_queue import enqueue_team, enqueue_agent
from pathlib import Path
home = Path(os.environ["HOMEDIR"])
enqueue_team("research_team", "probe prompt", parallelism=2, model="ail-compound", home=home)
enqueue_agent("researcher", "lookup", "who is lope", model="ail-compound", home=home)
"#,
        )
        .env("PY_PATH", py_path.as_os_str())
        .env("HOMEDIR", dir.path())
        .status()
        .expect("failed to run python3");
    assert!(status.success(), "python enqueue exited non-zero");

    // Read back via Rust.
    let entries = load_queue(dir.path()).expect("rust load_queue failed");
    assert_eq!(entries.len(), 2, "expected 2 entries, got {entries:?}");

    match &entries[0] {
        QueueEntry::Team { req, .. } => {
            assert_eq!(req.team, "research_team");
            assert_eq!(req.prompt, "probe prompt");
            assert_eq!(req.parallelism, Some(2));
            assert_eq!(req.model.as_deref(), Some("ail-compound"));
        }
        _ => panic!("first entry was not Team: {:?}", entries[0]),
    }
    match &entries[1] {
        QueueEntry::Agent { req, .. } => {
            assert_eq!(req.name, "researcher");
            assert_eq!(req.task, "lookup");
            assert_eq!(req.prompt, "who is lope");
        }
        _ => panic!("second entry was not Agent: {:?}", entries[1]),
    }
}
