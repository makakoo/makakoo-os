//! Safe archiving of generated agent runtime projects.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SlotRuntimeOnly {
    #[serde(default)]
    runtime: Option<RuntimeOnly>,
}

#[derive(Debug, Deserialize)]
struct RuntimeOnly {
    project_dir: PathBuf,
}

#[derive(Debug)]
pub(crate) struct RuntimeArchivePlan {
    pub project_dir: Option<PathBuf>,
    pub managed_source: Option<PathBuf>,
    pub warning: Option<String>,
}

pub(crate) fn plan(
    home: &Path,
    slot_id: &str,
    toml_body: &str,
) -> Result<RuntimeArchivePlan, String> {
    let parsed: SlotRuntimeOnly = match toml::from_str(toml_body) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(RuntimeArchivePlan {
                project_dir: None,
                managed_source: None,
                warning: Some(format!(
                    "slot TOML is malformed ({error}); runtime metadata could not be trusted, so only TOML and data were archived; inspect the archived TOML for any generated runtime that needs manual cleanup"
                )),
            });
        }
    };
    let Some(runtime) = parsed.runtime else {
        return Ok(RuntimeArchivePlan {
            project_dir: None,
            managed_source: None,
            warning: None,
        });
    };
    let source = runtime.project_dir;
    if !source.exists() {
        return Ok(RuntimeArchivePlan {
            project_dir: Some(source),
            managed_source: None,
            warning: None,
        });
    }
    let canonical = source
        .canonicalize()
        .map_err(|error| format!("resolve runtime project {}: {error}", source.display()))?;
    let managed = [home.join("agents-dsh"), home.join("agents-flue")]
        .into_iter()
        .filter(|root| root.exists())
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical == root.join(slot_id));
    Ok(RuntimeArchivePlan {
        project_dir: Some(source),
        managed_source: managed.then_some(canonical),
        warning: None,
    })
}

pub(crate) fn archive(
    plan: &RuntimeArchivePlan,
    archive_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(source) = plan.managed_source.as_ref() else {
        return Ok(None);
    };
    let destination = archive_dir.join("runtime");
    std::fs::rename(source, &destination).map_err(|error| {
        format!(
            "move runtime project {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_managed_slot_directory_is_archivable() {
        let tmp = tempfile::tempdir().unwrap();
        let managed = tmp.path().join("agents-dsh/researcher");
        std::fs::create_dir_all(&managed).unwrap();
        let body = format!(
            "[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
            managed
        );
        let managed_plan = plan(tmp.path(), "researcher", &body).unwrap();
        assert!(managed_plan.managed_source.is_some());

        let external = tmp.path().join("custom/researcher");
        std::fs::create_dir_all(&external).unwrap();
        let body = format!(
            "[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
            external
        );
        let external_plan = plan(tmp.path(), "researcher", &body).unwrap();
        assert!(external_plan.managed_source.is_none());
        assert_eq!(
            external_plan.project_dir.as_deref(),
            Some(external.as_path())
        );
    }

    #[test]
    fn malformed_slot_without_runtime_stays_destroyable() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = plan(tmp.path(), "broken", "slot_id =").unwrap();
        assert!(plan.managed_source.is_none());
        assert!(plan.warning.unwrap().contains("malformed"));
    }

    #[test]
    fn malformed_slot_with_runtime_table_stays_destroyable() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = plan(
            tmp.path(),
            "broken",
            "slot_id = \"broken\"\n[runtime]\nproject_dir =",
        )
        .unwrap();
        assert!(plan.managed_source.is_none());
        assert!(plan.warning.unwrap().contains("could not be trusted"));
    }
}
