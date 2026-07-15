//! Brain source registry and OKF interchange commands.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use makakoo_core::brain_sources_registry::{config_path, BrainSourcesRegistry};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use makakoo_core::brain_sources_registry::{
    config_backup_path, config_recovery_marker_path, config_temporary_path,
    REGISTRY_RECOVERY_MARKER_PREFIX,
};
#[cfg(test)]
use std::fs;

use crate::cli::BrainCmd;
use crate::context::CliContext;
use crate::okf::{export_bundle, validate_bundle, ExportOptions};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrainSourcesFile {
    #[serde(default = "default_source_name")]
    canonical: String,
    #[serde(default = "default_source_name")]
    default: String,
    #[serde(default)]
    sources: Vec<BrainSourceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrainSourceEntry {
    name: String,
    #[serde(default)]
    role: String,
    #[serde(rename = "type", default = "default_source_type")]
    source_type: String,
    path: String,
    #[serde(default)]
    writable: bool,
}

fn default_source_name() -> String {
    "default".to_string()
}

fn default_source_type() -> String {
    "plain".to_string()
}

fn normalize_source_type(source_type: &str) -> String {
    let normalized = source_type.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        default_source_type()
    } else {
        normalized
    }
}

fn is_okf_source_type(source_type: &str) -> bool {
    source_type.trim().eq_ignore_ascii_case("okf")
}

fn validate_source_type(source_type: &str) -> anyhow::Result<()> {
    if matches!(source_type, "logseq" | "obsidian" | "plain" | "okf") {
        Ok(())
    } else {
        bail!(
            "unsupported brain source type {source_type:?}; expected logseq, obsidian, plain, or okf"
        )
    }
}

impl BrainSourcesFile {
    fn default_for(home: &Path) -> Self {
        Self {
            canonical: "default".to_string(),
            default: "default".to_string(),
            sources: vec![BrainSourceEntry {
                name: "default".to_string(),
                role: "canonical".to_string(),
                source_type: "logseq".to_string(),
                path: home.join("data/Brain").to_string_lossy().into_owned(),
                writable: true,
            }],
        }
    }

    fn normalize(&mut self, home: &Path) -> anyhow::Result<()> {
        self.canonical = "default".to_string();
        self.default = "default".to_string();
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.name == "default")
        {
            source.role = "canonical".to_string();
            source.source_type = "logseq".to_string();
            source.path = home.join("data/Brain").to_string_lossy().into_owned();
            source.writable = true;
        } else {
            let mut canonical = Self::default_for(home).sources;
            self.sources.insert(0, canonical.remove(0));
        }
        for source in &mut self.sources {
            if source.name != "default" {
                source.role = "enrichment".to_string();
                source.source_type = normalize_source_type(&source.source_type);
                validate_source_type(&source.source_type)?;
                if is_okf_source_type(&source.source_type) {
                    source.writable = false;
                }
            }
        }
        Ok(())
    }
}

pub fn run(ctx: &CliContext, command: BrainCmd) -> anyhow::Result<i32> {
    match command {
        BrainCmd::List { json } => list(ctx, json),
        BrainCmd::Add {
            name,
            source_type,
            path,
            read_only,
            writable,
        } => add(ctx, &name, &source_type, &path, read_only, writable),
        BrainCmd::Remove { name } => remove(ctx, &name),
        BrainCmd::Export {
            format,
            source,
            out,
            include_journals,
            no_auto_memory,
            public,
            force,
            json,
        } => export(
            ctx,
            &format,
            &source,
            &out,
            include_journals,
            !no_auto_memory,
            public,
            force,
            json,
        ),
        BrainCmd::Validate { bundle, json } => validate(&bundle, json),
    }
}

fn list(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    let registry_file = BrainSourcesRegistry::acquire(ctx.home())?;
    let mut registry = load_registry_locked(&registry_file, ctx.home())?;
    registry.normalize(ctx.home())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&registry)?);
        return Ok(0);
    }
    println!(
        "Brain sources ({} total, canonical: default):",
        registry.sources.len()
    );
    for source in registry.sources {
        let canonical = if source.name == "default" {
            " (canonical)"
        } else {
            ""
        };
        let mode = if source.writable {
            "writable"
        } else {
            "read-only"
        };
        println!(
            "  [{:9}] {}{}  role={}\n              {} ({})",
            source.source_type, source.name, canonical, source.role, source.path, mode
        );
    }
    Ok(0)
}

fn add(
    ctx: &CliContext,
    name: &str,
    source_type: &str,
    path: &Path,
    read_only: bool,
    writable: bool,
) -> anyhow::Result<i32> {
    validate_source_name(name)?;
    if name == "default" {
        bail!("the canonical 'default' source is fixed and cannot be replaced");
    }
    let source_type = normalize_source_type(source_type);
    validate_source_type(&source_type)?;
    let root = path
        .canonicalize()
        .with_context(|| format!("source path does not exist: {}", path.display()))?;
    if !root.is_dir() {
        bail!("source path is not a directory: {}", root.display());
    }
    if is_okf_source_type(&source_type) {
        let report = validate_bundle(&root)?;
        if !report.conformant() {
            bail!(
                "refusing invalid OKF bundle: {} error(s); run 'makakoo brain validate {} --json'",
                report.errors.len(),
                root.display()
            );
        }
    }
    let registry_file = BrainSourcesRegistry::acquire(ctx.home())?;
    let mut registry = load_registry_locked(&registry_file, ctx.home())?;
    registry.normalize(ctx.home())?;
    reject_source_overlap(&registry, name, &root, ctx.home())?;
    let entry = BrainSourceEntry {
        name: name.to_string(),
        role: "enrichment".to_string(),
        source_type: source_type.clone(),
        path: root.to_string_lossy().into_owned(),
        writable: !is_okf_source_type(&source_type) && writable && !read_only,
    };
    if let Some(existing) = registry
        .sources
        .iter_mut()
        .find(|source| source.name == name)
    {
        *existing = entry;
    } else {
        registry.sources.push(entry);
    }
    registry
        .sources
        .sort_by(|left, right| left.name.cmp(&right.name));
    write_registry(&registry_file, &registry)?;
    println!(
        "added brain source {name:?} ({source_type}) at {}{}",
        root.display(),
        if is_okf_source_type(&source_type) || read_only || !writable {
            " [read-only]"
        } else {
            ""
        }
    );
    Ok(0)
}

fn remove(ctx: &CliContext, name: &str) -> anyhow::Result<i32> {
    if name == "default" {
        bail!("cannot remove canonical source 'default'");
    }
    let registry_file = BrainSourcesRegistry::acquire(ctx.home())?;
    let mut registry = load_registry_locked(&registry_file, ctx.home())?;
    registry.normalize(ctx.home())?;
    let before = registry.sources.len();
    registry.sources.retain(|source| source.name != name);
    if registry.sources.len() == before {
        bail!("no brain source named {name:?}");
    }
    write_registry(&registry_file, &registry)?;
    println!("removed brain source {name:?}; files were not deleted");
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn export(
    ctx: &CliContext,
    format: &str,
    source_name: &str,
    output: &Path,
    include_journals: bool,
    include_auto_memory: bool,
    public_only: bool,
    force: bool,
    json: bool,
) -> anyhow::Result<i32> {
    if format != "okf" {
        bail!("unsupported export format {format:?}; expected 'okf'");
    }
    let source = {
        let registry_file = BrainSourcesRegistry::acquire(ctx.home())?;
        let mut registry = load_registry_locked(&registry_file, ctx.home())?;
        registry.normalize(ctx.home())?;
        registry
            .sources
            .iter()
            .find(|source| source.name == source_name)
            .cloned()
            .with_context(|| format!("no brain source named {source_name:?}"))?
    };
    if is_okf_source_type(&source.source_type) {
        bail!(
            "source {source_name:?} is already an OKF bundle; use or copy its original directory"
        );
    }
    let options = ExportOptions {
        home: ctx.home().to_path_buf(),
        source_name: source.name.clone(),
        source_type: source.source_type.clone(),
        source_root: expand_source_path(&source.path, ctx.home()),
        output: output.to_path_buf(),
        include_journals,
        include_auto_memory: include_auto_memory && source.name == "default",
        public_only,
        force,
    };
    let report = export_bundle(&options)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "OKF v{} export complete: {} concepts ({} pages, {} memories, {} journals) -> {}",
            report.version,
            report.concepts,
            report.pages,
            report.memories,
            report.journals,
            report.output
        );
        if report.skipped_private > 0 {
            println!(
                "public filter skipped {} document(s) without visibility: public",
                report.skipped_private
            );
        }
    }
    Ok(0)
}

fn validate(bundle: &Path, json: bool) -> anyhow::Result<i32> {
    let report = validate_bundle(bundle)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "OKF v{} validation: {} concepts, {} index files, {} log files",
            report.version, report.concepts, report.indexes, report.logs
        );
        for error in &report.errors {
            println!("ERROR {}: {}", error.path, error.message);
        }
        for warning in &report.warnings {
            println!("WARN  {}: {}", warning.path, warning.message);
        }
        println!(
            "{}",
            if report.conformant() {
                "CONFORMANT"
            } else {
                "NOT_CONFORMANT"
            }
        );
    }
    Ok(if report.conformant() { 0 } else { 1 })
}

fn load_registry_locked(
    registry_file: &BrainSourcesRegistry,
    home: &Path,
) -> anyhow::Result<BrainSourcesFile> {
    let Some(raw) = registry_file.read_text()? else {
        return Ok(BrainSourcesFile::default_for(home));
    };
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "brain source config is invalid JSON: {}",
            config_path(home).display()
        )
    })
}

#[cfg(test)]
fn load_registry(home: &Path) -> anyhow::Result<BrainSourcesFile> {
    let registry_file = BrainSourcesRegistry::acquire(home)?;
    load_registry_locked(&registry_file, home)
}

fn write_registry(
    registry_file: &BrainSourcesRegistry,
    registry: &BrainSourcesFile,
) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(registry)? + "\n";
    registry_file.write_text(&body)
}

fn validate_source_name(name: &str) -> anyhow::Result<()> {
    let valid = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").expect("source name regex");
    if !valid.is_match(name) {
        bail!("invalid source name {name:?}; use 1-64 letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn expand_source_path(raw: &str, home: &Path) -> PathBuf {
    let expanded = expand_environment_variables(raw, home);
    if expanded == "~" {
        return dirs::home_dir().unwrap_or_else(|| home.to_path_buf());
    }
    if let Some(rest) = expanded.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| home.to_path_buf())
            .join(rest);
    }
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}

fn expand_environment_variables(raw: &str, home: &Path) -> String {
    let variables = Regex::new(
        r"\$(?:\{(?P<braced>[A-Za-z_][A-Za-z0-9_]*)\}|(?P<plain>[A-Za-z_][A-Za-z0-9_]*))",
    )
    .expect("environment variable regex");
    variables
        .replace_all(raw, |captures: &regex::Captures<'_>| {
            let name = captures
                .name("braced")
                .or_else(|| captures.name("plain"))
                .expect("environment variable capture")
                .as_str();
            if name == "MAKAKOO_HOME" || name == "HARVEY_HOME" {
                return home.to_string_lossy().into_owned();
            }
            std::env::var(name).unwrap_or_else(|_| captures[0].to_string())
        })
        .into_owned()
}

fn reject_source_overlap(
    registry: &BrainSourcesFile,
    candidate_name: &str,
    candidate_root: &Path,
    home: &Path,
) -> anyhow::Result<()> {
    for source in &registry.sources {
        if source.name == candidate_name {
            continue;
        }
        let existing_root = expand_source_path(&source.path, home);
        if source_paths_overlap(candidate_root, &existing_root) {
            bail!(
                "brain source root overlaps existing source {:?}: {}",
                source.name,
                existing_root.display()
            );
        }
    }
    Ok(())
}

fn source_paths_overlap(left: &Path, right: &Path) -> bool {
    let left = left
        .canonicalize()
        .unwrap_or_else(|_| normalize_source_path(left));
    let right = right
        .canonicalize()
        .unwrap_or_else(|_| normalize_source_path(right));
    left.starts_with(&right) || right.starts_with(&left)
}

fn normalize_source_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_okf_is_read_only_and_remove_never_deletes_files() {
        let home = tempdir().unwrap();
        let bundle = tempdir().unwrap();
        fs::write(
            bundle.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        )
        .unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());
        assert_eq!(
            add(&ctx, "catalog", "OKF", bundle.path(), false, true).unwrap(),
            0
        );
        let registry = load_registry(home.path()).unwrap();
        let catalog = registry
            .sources
            .iter()
            .find(|source| source.name == "catalog")
            .unwrap();
        assert!(!catalog.writable);
        assert_eq!(catalog.role, "enrichment");
        assert_eq!(catalog.source_type, "okf");
        assert_eq!(remove(&ctx, "catalog").unwrap(), 0);
        assert!(bundle.path().join("concept.md").exists());
    }

    #[test]
    fn corrupt_registry_is_not_overwritten() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        fs::write(config_path(home.path()), "{broken").unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());
        let source = tempdir().unwrap();
        let error = add(&ctx, "notes", "plain", source.path(), true, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid JSON"));
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            "{broken"
        );
    }

    #[test]
    fn enrichment_sources_default_to_read_only() {
        let home = tempdir().unwrap();
        let source = tempdir().unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());
        add(&ctx, "notes", "plain", source.path(), false, false).unwrap();
        let registry = load_registry(home.path()).unwrap();
        assert!(
            !registry
                .sources
                .iter()
                .find(|entry| entry.name == "notes")
                .unwrap()
                .writable
        );
    }

    #[test]
    fn add_rejects_unknown_source_type_before_writing() {
        let home = tempdir().unwrap();
        let source = tempdir().unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());

        let error = add(&ctx, "typo", "plian", source.path(), true, false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsupported brain source type"));
        assert!(!config_path(home.path()).exists());
    }

    #[test]
    fn add_rejects_source_root_overlap_before_writing() {
        let home = tempdir().unwrap();
        let brain = home.path().join("data/Brain");
        fs::create_dir_all(&brain).unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());

        let error = add(&ctx, "duplicate", "plain", &brain, true, false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("overlaps existing source"));
        assert!(!config_path(home.path()).exists());
    }

    #[test]
    fn source_path_expansion_supports_shell_style_environment_variables() {
        let home = Path::new("/makakoo-home");
        let process_home = std::env::var("HOME").unwrap();

        assert_eq!(
            expand_source_path("$HOME/notes", home),
            PathBuf::from(&process_home).join("notes")
        );
        assert_eq!(
            expand_source_path("${HOME}/notes", home),
            PathBuf::from(process_home).join("notes")
        );
        assert_eq!(
            expand_source_path("$HARVEY_HOME/data/Brain", home),
            home.join("data/Brain")
        );
    }

    #[test]
    fn interrupted_registry_replacement_recovers_backup() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let expected = serde_json::to_string(&BrainSourcesFile::default_for(home.path())).unwrap();
        fs::write(config_backup_path(home.path()), &expected).unwrap();
        fs::write(
            config_recovery_marker_path(home.path()),
            format!("{REGISTRY_RECOVERY_MARKER_PREFIX}{expected}"),
        )
        .unwrap();
        let registry = load_registry(home.path()).unwrap();
        assert_eq!(registry.sources[0].name, "default");
        assert!(config_path(home.path()).exists());
        assert!(!config_backup_path(home.path()).exists());
        assert!(!config_recovery_marker_path(home.path()).exists());
    }

    #[test]
    fn registry_recovery_preserves_unowned_backup_collision() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        fs::write(config_backup_path(home.path()), "unrelated").unwrap();
        let error = load_registry(home.path()).unwrap_err().to_string();
        assert!(error.contains("unowned brain source recovery artifacts"));
        assert_eq!(
            fs::read_to_string(config_backup_path(home.path())).unwrap(),
            "unrelated"
        );
        assert!(!config_path(home.path()).exists());
    }

    #[test]
    fn registry_recovery_preserves_backup_when_primary_is_not_owned_promotion() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let primary = serde_json::to_string(&BrainSourcesFile::default_for(home.path())).unwrap();
        let intended = "{\"canonical\":\"default\",\"sources\":[]}";
        fs::write(config_path(home.path()), &primary).unwrap();
        fs::write(config_backup_path(home.path()), "known-good").unwrap();
        fs::write(
            config_recovery_marker_path(home.path()),
            format!("{REGISTRY_RECOVERY_MARKER_PREFIX}{intended}"),
        )
        .unwrap();

        let error = load_registry(home.path()).unwrap_err().to_string();
        assert!(error.contains("does not match the owned transaction"));
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            primary
        );
        assert_eq!(
            fs::read_to_string(config_backup_path(home.path())).unwrap(),
            "known-good"
        );
    }

    #[test]
    fn marker_only_recovery_reconstructs_initial_registry() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let intended = serde_json::to_string_pretty(&BrainSourcesFile::default_for(home.path()))
            .unwrap()
            + "\n";
        fs::write(
            config_recovery_marker_path(home.path()),
            format!("{REGISTRY_RECOVERY_MARKER_PREFIX}{intended}"),
        )
        .unwrap();

        let registry = load_registry(home.path()).unwrap();

        assert_eq!(registry.sources[0].name, "default");
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            intended
        );
        assert!(!config_recovery_marker_path(home.path()).exists());
    }

    #[test]
    fn marker_before_temp_keeps_existing_registry() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let existing = serde_json::to_string_pretty(&BrainSourcesFile::default_for(home.path()))
            .unwrap()
            + "\n";
        let intended = "{\"canonical\":\"default\",\"sources\":[]}";
        fs::write(config_path(home.path()), &existing).unwrap();
        fs::write(
            config_recovery_marker_path(home.path()),
            format!("{REGISTRY_RECOVERY_MARKER_PREFIX}{intended}"),
        )
        .unwrap();

        let registry = load_registry(home.path()).unwrap();

        assert_eq!(registry.sources[0].name, "default");
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            existing
        );
        assert!(!config_recovery_marker_path(home.path()).exists());
    }

    #[test]
    fn temp_only_recovery_reconstructs_initial_registry() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let intended = serde_json::to_string_pretty(&BrainSourcesFile::default_for(home.path()))
            .unwrap()
            + "\n";
        fs::write(
            config_recovery_marker_path(home.path()),
            format!("{REGISTRY_RECOVERY_MARKER_PREFIX}{intended}"),
        )
        .unwrap();
        fs::write(config_temporary_path(home.path()), "partial").unwrap();

        let registry = load_registry(home.path()).unwrap();

        assert_eq!(registry.sources[0].name, "default");
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            intended
        );
        assert!(!config_temporary_path(home.path()).exists());
        assert!(!config_recovery_marker_path(home.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_registry_symlink_fails_closed() {
        use std::os::unix::fs::symlink;

        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join("config")).unwrap();
        let path = config_path(home.path());
        symlink(home.path().join("missing-registry"), &path).unwrap();

        let error = load_registry(home.path()).unwrap_err().to_string();
        assert!(error.contains("not a regular file"));
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[test]
    fn export_refuses_to_flatten_an_existing_okf_source() {
        let home = tempdir().unwrap();
        let bundle = tempdir().unwrap();
        fs::write(
            bundle.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        )
        .unwrap();
        let ctx = CliContext::for_home(home.path().to_path_buf());
        add(&ctx, "catalog", "okf", bundle.path(), false, false).unwrap();
        let error = export(
            &ctx,
            "okf",
            "catalog",
            &home.path().join("out"),
            false,
            false,
            false,
            false,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("already an OKF bundle"));
    }

    #[test]
    fn normalize_closes_uppercase_okf_registry_safety_bypass() {
        let home = tempdir().unwrap();
        let bundle = tempdir().unwrap();
        let mut registry = BrainSourcesFile {
            canonical: "elsewhere".to_string(),
            default: "elsewhere".to_string(),
            sources: vec![BrainSourceEntry {
                name: "catalog".to_string(),
                role: "canonical".to_string(),
                source_type: " OKF ".to_string(),
                path: bundle.path().to_string_lossy().into_owned(),
                writable: true,
            }],
        };

        registry.normalize(home.path()).unwrap();

        let catalog = registry
            .sources
            .iter()
            .find(|source| source.name == "catalog")
            .unwrap();
        assert_eq!(catalog.source_type, "okf");
        assert_eq!(catalog.role, "enrichment");
        assert!(!catalog.writable);
    }
}
