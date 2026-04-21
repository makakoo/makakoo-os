//! Every reference adapter under `plugins-core/adapters/*/adapter.toml`
//! must parse and validate cleanly. If you're adding a new built-in
//! adapter, drop it in that dir and the test picks it up automatically.

use std::path::PathBuf;

use makakoo_core::adapter::Manifest;

fn reference_adapters_dir() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .expect("workspace root")
        .join("plugins-core/adapters")
}

#[test]
fn every_reference_adapter_parses() {
    let root = reference_adapters_dir();
    assert!(
        root.is_dir(),
        "expected reference adapters dir at {}",
        root.display()
    );

    let mut names = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("adapter.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = Manifest::load(&manifest_path)
            .unwrap_or_else(|e| panic!("failed to load {}: {e}", manifest_path.display()));
        names.push(manifest.adapter.name.clone());
    }

    names.sort();
    assert_eq!(
        names,
        vec![
            "claude-api".to_string(),
            "hermes".to_string(),
            "llama-cpp-server".to_string(),
            "ollama".to_string(),
            "openai-api".to_string(),
            "openclaw".to_string(),
            "openrouter".to_string(),
            "pi".to_string(),
            "switchailocal".to_string(),
            "tytus-cli".to_string(),
            "tytus-pod".to_string(),
        ],
        "all reference adapters must parse; got {:?}",
        names
    );
}

#[test]
fn reference_adapters_have_unique_canonical_hashes() {
    let root = reference_adapters_dir();
    let mut hashes = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        let manifest_path = entry.path().join("adapter.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = Manifest::load(&manifest_path).unwrap();
        hashes.push((manifest.adapter.name.clone(), manifest.canonical_hash()));
    }
    hashes.sort();
    // Every adapter must have a unique hash — trust ledger relies on this.
    for window in hashes.windows(2) {
        assert_ne!(window[0].1, window[1].1, "hash collision: {window:?}");
    }
    assert_eq!(hashes.len(), 11);
}
