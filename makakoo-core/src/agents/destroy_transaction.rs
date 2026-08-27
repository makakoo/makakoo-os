//! Transactional staging for agent destroy archives.

use std::path::{Path, PathBuf};

use super::runtime_archive::RuntimeArchivePlan;

#[derive(Debug)]
pub(crate) struct StagedArchive {
    pub archived_runtime_dir: Option<PathBuf>,
    pub archived_data_dir: Option<PathBuf>,
}

pub(crate) fn stage(
    plan: &RuntimeArchivePlan,
    archive_dir: &Path,
    data_src: &Path,
) -> Result<StagedArchive, String> {
    let archived_runtime_dir = match super::runtime_archive::archive(plan, archive_dir) {
        Ok(path) => path,
        Err(error) => {
            let cleanup = match std::fs::remove_dir(archive_dir) {
                Ok(()) => "empty archive directory removed".to_string(),
                Err(cleanup) => format!(
                    "remove empty archive directory {} manually ({cleanup})",
                    archive_dir.display()
                ),
            };
            return Err(format!("stage runtime: {error}. {cleanup}"));
        }
    };
    let data_dst = archive_dir.join("data");
    let data_existed = data_src.exists();
    let data_result = if data_existed {
        std::fs::rename(data_src, &data_dst)
    } else {
        std::fs::create_dir_all(&data_dst)
    };
    if let Err(error) = data_result {
        let staged = StagedArchive {
            archived_runtime_dir,
            // The rename failed, so the destination never became staged data.
            // Recording it as archived would make rollback emit a bogus `mv`.
            archived_data_dir: None,
        };
        let recovery = rollback(plan, archive_dir, data_src, &staged);
        return Err(format!(
            "stage data {}: {error}. {recovery}",
            data_src.display()
        ));
    }
    Ok(StagedArchive {
        archived_runtime_dir,
        archived_data_dir: data_existed.then_some(data_dst),
    })
}

pub(crate) fn rollback(
    plan: &RuntimeArchivePlan,
    archive_dir: &Path,
    data_src: &Path,
    staged: &StagedArchive,
) -> String {
    let mut failures = Vec::new();
    let data_dst = archive_dir.join("data");
    if staged.archived_data_dir.is_some() {
        if let Err(error) = std::fs::rename(&data_dst, data_src) {
            failures.push(format!(
                "restore data with `mv {} {}` ({error})",
                data_dst.display(),
                data_src.display()
            ));
        }
    } else if let Err(error) = std::fs::remove_dir(&data_dst) {
        if error.kind() != std::io::ErrorKind::NotFound {
            failures.push(format!("remove {} ({error})", data_dst.display()));
        }
    }
    if staged.archived_runtime_dir.is_some() {
        if let (Some(source), Some(archived)) = (
            plan.managed_source.as_ref(),
            staged.archived_runtime_dir.as_ref(),
        ) {
            if let Err(error) = std::fs::rename(archived, source) {
                failures.push(format!(
                    "restore runtime with `mv {} {}` ({error})",
                    archived.display(),
                    source.display()
                ));
            }
        }
    }
    if let Err(error) = std::fs::remove_dir(archive_dir) {
        if error.kind() != std::io::ErrorKind::NotFound
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
        {
            failures.push(format!("remove {} ({error})", archive_dir.display()));
        }
    }
    if failures.is_empty() {
        "all staged moves rolled back; registry and runtime remain intact".into()
    } else {
        format!("automatic rollback incomplete: {}", failures.join("; "))
    }
}

pub(crate) fn commit_registry(
    plan: &RuntimeArchivePlan,
    archive_dir: &Path,
    data_src: &Path,
    staged: &StagedArchive,
    toml_src: &Path,
    toml_dst: &Path,
) -> Result<(), String> {
    if let Err(error) = std::fs::rename(toml_src, toml_dst) {
        let recovery = rollback(plan, archive_dir, data_src, staged);
        return Err(format!(
            "move registry TOML {} to {}: {error}. {recovery}",
            toml_src.display(),
            toml_dst.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_restores_runtime_and_data() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("agents-dsh/researcher");
        let data = tmp.path().join("data/agents/researcher");
        let archive = tmp.path().join("archive/agents/researcher-1");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        let body = format!(
            "[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
            runtime
        );
        let plan = super::super::runtime_archive::plan(tmp.path(), "researcher", &body).unwrap();
        let staged = stage(&plan, &archive, &data).unwrap();
        assert!(!runtime.exists());
        assert!(!data.exists());
        let result = rollback(&plan, &archive, &data, &staged);
        assert!(result.contains("rolled back"));
        assert!(runtime.exists());
        assert!(data.exists());
        assert!(!archive.join("runtime").exists());
    }

    #[test]
    fn registry_commit_failure_rolls_back_all_staged_state() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("agents-dsh/researcher");
        let data = tmp.path().join("data/agents/researcher");
        let toml = tmp.path().join("config/agents/researcher.toml");
        let archive = tmp.path().join("archive/agents/researcher-2");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(toml.parent().unwrap()).unwrap();
        std::fs::write(&toml, "slot_id = \"researcher\"\n").unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        let body = format!(
            "[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
            runtime
        );
        let plan = super::super::runtime_archive::plan(tmp.path(), "researcher", &body).unwrap();
        let staged = stage(&plan, &archive, &data).unwrap();
        let toml_dst = archive.join("researcher.toml");
        std::fs::create_dir(&toml_dst).unwrap();
        let error = commit_registry(&plan, &archive, &data, &staged, &toml, &toml_dst).unwrap_err();
        assert!(error.contains("rolled back"));
        assert!(toml.exists());
        assert!(runtime.exists());
        assert!(data.exists());
        assert!(!archive.join("runtime").exists());
    }

    #[test]
    fn failed_data_stage_does_not_claim_destination_was_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data/agents/researcher");
        let archive = tmp.path().join("archive/agents/researcher-3");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("state"), "x").unwrap();
        std::fs::create_dir_all(archive.join("data")).unwrap();
        std::fs::write(archive.join("data/existing"), "x").unwrap();
        let plan = super::super::runtime_archive::plan(
            tmp.path(),
            "researcher",
            "slot_id = \"researcher\"\n",
        )
        .unwrap();
        let error = stage(&plan, &archive, &data).unwrap_err();
        assert!(!error.contains("restore data with `mv"));
        assert!(data.exists());
    }
}
