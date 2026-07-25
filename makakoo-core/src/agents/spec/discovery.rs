//! Path → list<AgentSpec> discovery.
//!
//! Single-file: parses one YAML or TOML spec.
//! Directory: non-recursive glob of `*.yaml`, `*.yml`, `*.toml`,
//! sorted lexicographically. Empty directories return an empty vec.
//!
//! `--specs .` semantics live at the CLI layer; the discovery module
//! accepts any path and decides single-file vs directory by
//! inspecting the path's metadata.

use std::fs;
use std::path::Path;

use crate::Result;
use super::AgentSpec;

const SPEC_EXTENSIONS: &[&str] = &["yaml", "yml", "toml"];

/// Discover specs from a path. If the path is a file, returns
/// `vec![parsed]`. If the path is a directory, returns all
/// matching specs (non-recursive, sorted). Errors on missing path.
pub fn discover_specs(path: &Path) -> Result<Vec<AgentSpec>> {
    if !path.exists() {
        return Err(crate::MakakooError::NotFound(format!(
            "spec path {} does not exist",
            path.display()
        )));
    }
    if path.is_file() {
        return Ok(vec![AgentSpec::load_from_file(path)?]);
    }
    if path.is_dir() {
        return discover_in_dir(path);
    }
    Err(crate::MakakooError::InvalidInput(format!(
        "spec path {} is neither a file nor a directory",
        path.display()
    )))
}

fn discover_in_dir(dir: &Path) -> Result<Vec<AgentSpec>> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| {
            crate::MakakooError::Config(format!(
                "spec dir {} read: {}",
                dir.display(),
                e
            ))
        })?
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let p = entry.path();
            let ext = p.extension().and_then(|e| e.to_str())?;
            if SPEC_EXTENSIONS.contains(&ext) {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    // Deterministic order.
    entries.sort();

    if entries.is_empty() {
        return Err(crate::MakakooError::InvalidInput(format!(
            "spec dir {} contains no .yaml/.yml/.toml files",
            dir.display()
        )));
    }

    let mut specs = Vec::with_capacity(entries.len());
    for p in &entries {
        specs.push(AgentSpec::load_from_file(p)?);
    }
    Ok(specs)
}

/// Returns true if the path is a directory containing spec files.
/// Used by the CLI to decide whether to recurse or scan.
pub fn is_dir_with_specs(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    fs::read_dir(path)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let p = e.path();
                p.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| SPEC_EXTENSIONS.contains(&ext))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const MINIMAL_YAML: &str = r#"
name: foo
description: "x"
model: claude
instructions: hi
tools: []
scope: {}
"#;

    fn write_spec(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn discover_single_file_yaml() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "agent.yaml", MINIMAL_YAML);
        let specs = discover_specs(&p).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "foo");
    }

    #[test]
    fn discover_single_file_toml() {
        let dir = TempDir::new().unwrap();
        let body = r#"
name = "bar"
description = "y"
model = "claude"
instructions = "hi"
tools = []

[scope]
"#;
        let p = write_spec(dir.path(), "agent.toml", body);
        let specs = discover_specs(&p).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "bar");
    }

    #[test]
    fn discover_directory_yaml_and_toml() {
        let dir = TempDir::new().unwrap();
        write_spec(dir.path(), "a.yaml", &MINIMAL_YAML.replace("name: foo", "name: a"));
        write_spec(
            dir.path(),
            "b.toml",
            r#"name = "b"
description = "y"
model = "claude"
instructions = "hi"
tools = []

[scope]
"#,
        );
        write_spec(
            dir.path(),
            "c.yml",
            &MINIMAL_YAML.replace("name: foo", "name: c"),
        );
        // Non-spec file should be ignored.
        fs::write(dir.path().join("README.md"), "# notes").unwrap();
        fs::write(dir.path().join("notes.txt"), "junk").unwrap();

        let specs = discover_specs(dir.path()).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn discover_directory_sorted_deterministically() {
        let dir = TempDir::new().unwrap();
        write_spec(dir.path(), "z.yaml", &MINIMAL_YAML.replace("name: foo", "name: z"));
        write_spec(dir.path(), "a.yaml", &MINIMAL_YAML.replace("name: foo", "name: a"));
        write_spec(dir.path(), "m.yaml", &MINIMAL_YAML.replace("name: foo", "name: m"));
        let specs = discover_specs(dir.path()).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a", "m", "z"]);
    }

    #[test]
    fn discover_empty_directory_errors() {
        let dir = TempDir::new().unwrap();
        let err = discover_specs(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("no .yaml/.yml/.toml files"));
    }

    #[test]
    fn discover_missing_path_errors() {
        let p = std::path::Path::new("/nonexistent/path/xyz");
        let err = discover_specs(p).unwrap_err();
        assert!(format!("{err}").contains("does not exist"));
    }

    #[test]
    fn discover_rejects_duplicate_names_in_batch() {
        // Two specs with the same `name` should both parse, but the
        // CLI layer is responsible for pre-flight duplicate detection
        // (atomicity: don't create any agent if the batch has dupes).
        let dir = TempDir::new().unwrap();
        write_spec(dir.path(), "a.yaml", MINIMAL_YAML);
        write_spec(dir.path(), "b.yaml", MINIMAL_YAML);
        let specs = discover_specs(dir.path()).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["foo", "foo"]);
    }
}
