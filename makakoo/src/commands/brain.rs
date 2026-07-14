//! Brain source registry and OKF interchange commands.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use regex::Regex;
use serde::{Deserialize, Serialize};

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

    fn normalize(&mut self, home: &Path) {
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
                if source.source_type == "okf" {
                    source.writable = false;
                }
            }
        }
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
        } => add(ctx, &name, &source_type, &path, read_only),
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
    let mut registry = load_registry(ctx.home())?;
    registry.normalize(ctx.home());
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
) -> anyhow::Result<i32> {
    validate_source_name(name)?;
    if name == "default" {
        bail!("the canonical 'default' source is fixed and cannot be replaced");
    }
    let root = path
        .canonicalize()
        .with_context(|| format!("source path does not exist: {}", path.display()))?;
    if !root.is_dir() {
        bail!("source path is not a directory: {}", root.display());
    }
    if source_type == "okf" {
        let report = validate_bundle(&root)?;
        if !report.conformant() {
            bail!(
                "refusing invalid OKF bundle: {} error(s); run 'makakoo brain validate {} --json'",
                report.errors.len(),
                root.display()
            );
        }
    }
    let mut registry = load_registry(ctx.home())?;
    registry.normalize(ctx.home());
    let entry = BrainSourceEntry {
        name: name.to_string(),
        role: "enrichment".to_string(),
        source_type: source_type.to_string(),
        path: root.to_string_lossy().into_owned(),
        writable: source_type != "okf" && !read_only,
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
    write_registry(ctx.home(), &registry)?;
    println!(
        "added brain source {name:?} ({source_type}) at {}{}",
        root.display(),
        if source_type == "okf" || read_only {
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
    let mut registry = load_registry(ctx.home())?;
    registry.normalize(ctx.home());
    let before = registry.sources.len();
    registry.sources.retain(|source| source.name != name);
    if registry.sources.len() == before {
        bail!("no brain source named {name:?}");
    }
    write_registry(ctx.home(), &registry)?;
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
    let mut registry = load_registry(ctx.home())?;
    registry.normalize(ctx.home());
    let source = registry
        .sources
        .iter()
        .find(|source| source.name == source_name)
        .with_context(|| format!("no brain source named {source_name:?}"))?;
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

fn config_path(home: &Path) -> PathBuf {
    home.join("config/brain_sources.json")
}

fn load_registry(home: &Path) -> anyhow::Result<BrainSourcesFile> {
    let path = config_path(home);
    if !path.exists() {
        return Ok(BrainSourcesFile::default_for(home));
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read brain source config {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("brain source config is invalid JSON: {}", path.display()))
}

fn write_registry(home: &Path, registry: &BrainSourcesFile) -> anyhow::Result<()> {
    let path = config_path(home);
    let parent = path.parent().context("brain source config has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".brain_sources.json.tmp-{}", std::process::id()));
    let body = serde_json::to_string_pretty(registry)? + "\n";
    fs::write(&temporary, body)?;
    if !path.exists() {
        fs::rename(&temporary, &path)?;
        return Ok(());
    }
    let backup = parent.join(format!(".brain_sources.json.backup-{}", std::process::id()));
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    fs::rename(&path, &backup)?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::rename(&backup, &path);
        let _ = fs::remove_file(&temporary);
        return Err(error).context("atomically replace brain source config");
    }
    fs::remove_file(backup)?;
    Ok(())
}

fn validate_source_name(name: &str) -> anyhow::Result<()> {
    let valid = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").expect("source name regex");
    if !valid.is_match(name) {
        bail!("invalid source name {name:?}; use 1-64 letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

fn expand_source_path(raw: &str, home: &Path) -> PathBuf {
    let home_text = home.to_string_lossy();
    let expanded = raw
        .replace("$MAKAKOO_HOME", &home_text)
        .replace("$HARVEY_HOME", &home_text);
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
            add(&ctx, "catalog", "okf", bundle.path(), false).unwrap(),
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
        let error = add(&ctx, "notes", "plain", source.path(), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid JSON"));
        assert_eq!(
            fs::read_to_string(config_path(home.path())).unwrap(),
            "{broken"
        );
    }
}
