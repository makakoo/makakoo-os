//! Open Knowledge Format v0.1 import/export boundary.
//!
//! Makakoo's Logseq Brain remains canonical. This module only produces and
//! validates portable OKF bundles, with no network or publishing behavior.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use fs2::FileExt;
use regex::{Captures, Regex};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest, Sha256};

const OKF_VERSION: &str = "0.1";
const RESERVED_FILENAMES: &[&str] = &["index.md", "log.md"];
const RECOVERY_MARKER_VERSION: &str = "makakoo-okf-recovery-v2";

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub home: PathBuf,
    pub source_name: String,
    pub source_type: String,
    pub source_root: PathBuf,
    pub output: PathBuf,
    pub include_journals: bool,
    pub include_auto_memory: bool,
    pub public_only: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub version: String,
    pub source: String,
    pub output: String,
    pub concepts: usize,
    pub pages: usize,
    pub memories: usize,
    pub journals: usize,
    pub skipped_private: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub version: String,
    pub bundle: String,
    pub concepts: usize,
    pub indexes: usize,
    pub logs: usize,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn conformant(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConceptKind {
    Page,
    Memory,
    Journal,
}

impl ConceptKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Page => "pages",
            Self::Memory => "memories",
            Self::Journal => "journals",
        }
    }

    fn default_type(self) -> &'static str {
        match self {
            Self::Page => "Knowledge Page",
            Self::Memory => "Memory",
            Self::Journal => "Journal Entry",
        }
    }
}

#[derive(Debug)]
struct SourceDocument {
    path: PathBuf,
    relative_path: String,
    kind: ConceptKind,
    raw: String,
}

#[derive(Debug)]
struct PreparedDocument {
    destination: String,
    title: String,
    description: String,
    metadata: BTreeMap<String, Value>,
    body: String,
    kind: ConceptKind,
    lookup_keys: Vec<String>,
}

pub fn export_bundle(options: &ExportOptions) -> anyhow::Result<ExportReport> {
    validate_export_paths(options)?;
    let _lock = lock_export_output(&options.output)?;
    validate_export_paths(options)?;
    recover_export_output(&options.output)?;
    validate_export_paths(options)?;
    let documents = collect_source_documents(options)?;
    let mut skipped_private = 0usize;
    let mut prepared = Vec::new();
    let mut used_destinations = HashSet::new();

    for document in documents {
        let (existing, body) = parse_optional_frontmatter(&document.raw)
            .with_context(|| format!("invalid frontmatter in {}", document.path.display()))?;
        let logseq = extract_logseq_properties(&body);
        if options.public_only && !is_public(&existing, &logseq) {
            skipped_private += 1;
            continue;
        }
        if options.public_only {
            if let Some(reason) = likely_secret(&document.raw) {
                bail!(
                    "public export refused: {} contains likely secret material ({reason})",
                    document.path.display()
                );
            }
        }

        let title = metadata_string(&existing, "title")
            .or_else(|| metadata_string(&existing, "name"))
            .or_else(|| first_h1(&body))
            .unwrap_or_else(|| display_stem(&document.path));
        let description = metadata_string(&existing, "description")
            .unwrap_or_else(|| derive_description(&body, &title));
        let base_slug = slugify(
            document
                .path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(&title),
        );
        let mut destination = format!("{}/{}.md", document.kind.directory(), base_slug);
        if !used_destinations.insert(destination.clone()) {
            destination = format!(
                "{}/{}-{:08x}.md",
                document.kind.directory(),
                base_slug,
                stable_hash(&document.relative_path) as u32
            );
            if !used_destinations.insert(destination.clone()) {
                bail!("duplicate OKF destination for {}", document.path.display());
            }
        }

        let mut metadata = mapping_to_btree(existing)?;
        let concept_type = metadata
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| nested_metadata_string(&metadata, "metadata", "type"))
            .or_else(|| logseq.get("type").cloned())
            .unwrap_or_else(|| document.kind.default_type().to_string());
        let tags = collect_tags(&metadata, &logseq);
        let timestamp = metadata
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| modified_timestamp(&document.path));
        metadata.insert("type".to_string(), Value::String(concept_type));
        metadata.insert("title".to_string(), Value::String(title.clone()));
        metadata.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
        metadata.entry("resource".to_string()).or_insert_with(|| {
            Value::String(format!(
                "makakoo://brain/{}/{}",
                percent_encode(&options.source_name),
                percent_encode(&document.relative_path)
            ))
        });
        metadata.insert(
            "tags".to_string(),
            Value::Sequence(tags.into_iter().map(Value::String).collect()),
        );
        metadata.insert("timestamp".to_string(), Value::String(timestamp));
        metadata.insert(
            "makakoo_source".to_string(),
            Value::String(options.source_name.clone()),
        );
        metadata.insert(
            "makakoo_source_path".to_string(),
            Value::String(document.relative_path.clone()),
        );

        let mut lookup_keys = vec![display_stem(&document.path), title.clone()];
        lookup_keys.sort();
        lookup_keys.dedup();
        prepared.push(PreparedDocument {
            destination,
            title,
            description,
            metadata,
            body,
            kind: document.kind,
            lookup_keys,
        });
    }

    prepared.sort_by(|left, right| left.destination.cmp(&right.destination));
    if prepared.is_empty() {
        bail!(
            "export produced no concepts{}",
            if options.public_only {
                "; mark documents with 'visibility: public' before using --public"
            } else {
                ""
            }
        );
    }
    let link_map = build_link_map(&prepared);
    let stage = staging_path(&options.output, "stage");
    write_recovery_marker(&options.output, "stage")?;
    if let Err(error) = fs::create_dir(&stage) {
        let _ = remove_recovery_marker(&options.output, "stage");
        return Err(error).with_context(|| format!("create staging dir {}", stage.display()));
    }

    let build_result = write_bundle(&stage, &prepared, &link_map, &options.source_name)
        .and_then(|()| sync_tree(&stage));
    if let Err(error) = build_result {
        let _ = cleanup_owned_stage(&options.output);
        return Err(error);
    }
    let stage_digest = match tree_digest(&stage) {
        Ok(digest) => digest,
        Err(error) => {
            let _ = cleanup_owned_stage(&options.output);
            return Err(error);
        }
    };
    commit_staged_directory(&stage, &options.output, options.force, &stage_digest)?;

    let pages = prepared
        .iter()
        .filter(|document| document.kind == ConceptKind::Page)
        .count();
    let memories = prepared
        .iter()
        .filter(|document| document.kind == ConceptKind::Memory)
        .count();
    let journals = prepared
        .iter()
        .filter(|document| document.kind == ConceptKind::Journal)
        .count();
    Ok(ExportReport {
        version: OKF_VERSION.to_string(),
        source: options.source_name.clone(),
        output: options.output.to_string_lossy().into_owned(),
        concepts: prepared.len(),
        pages,
        memories,
        journals,
        skipped_private,
    })
}

fn validate_export_paths(options: &ExportOptions) -> anyhow::Result<()> {
    if !options.source_root.is_dir() {
        bail!(
            "source root does not exist: {}",
            options.source_root.display()
        );
    }
    if path_entry_exists(&options.output)? {
        let metadata = fs::symlink_metadata(&options.output)?;
        if metadata.file_type().is_symlink() {
            bail!("output cannot be a symlink: {}", options.output.display());
        }
        if !metadata.file_type().is_dir() {
            bail!("output is not a directory: {}", options.output.display());
        }
        if directory_has_entries(&options.output)? && !options.force {
            bail!(
                "output directory is not empty: {} (pass --force to replace it)",
                options.output.display()
            );
        }
    }
    let source = canonical_or_absolute(&options.source_root)?;
    let output = canonicalize_existing_ancestor(&options.output)?;
    if output.starts_with(&source) || source.starts_with(&output) {
        bail!("output cannot overlap source root: {}", output.display());
    }
    if options.include_auto_memory {
        let auto_memory = canonical_or_absolute(&options.home.join("data/auto-memory"))?;
        if output.starts_with(&auto_memory) || auto_memory.starts_with(&output) {
            bail!(
                "output cannot overlap auto-memory source: {}",
                output.display()
            );
        }
    }
    Ok(())
}

fn collect_source_documents(options: &ExportOptions) -> anyhow::Result<Vec<SourceDocument>> {
    let mut documents = Vec::new();
    if options.source_name == "default" {
        collect_markdown(
            &options.source_root.join("pages"),
            &options.source_root,
            ConceptKind::Page,
            false,
            &mut documents,
        )?;
        if options.include_journals {
            collect_markdown(
                &options.source_root.join("journals"),
                &options.source_root,
                ConceptKind::Journal,
                false,
                &mut documents,
            )?;
        }
        if options.include_auto_memory {
            let auto_memory = options.home.join("data").join("auto-memory");
            collect_markdown(
                &auto_memory,
                &auto_memory,
                ConceptKind::Memory,
                false,
                &mut documents,
            )?;
        }
    } else {
        collect_markdown(
            &options.source_root,
            &options.source_root,
            ConceptKind::Page,
            options.source_type.eq_ignore_ascii_case("okf"),
            &mut documents,
        )?;
    }
    documents.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(documents)
}

fn collect_markdown(
    directory: &Path,
    relative_root: &Path,
    kind: ConceptKind,
    skip_okf_reserved: bool,
    output: &mut Vec<SourceDocument>,
) -> anyhow::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("read source directory {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "logseq" | "bak" | ".git") {
                continue;
            }
            collect_markdown(&path, relative_root, kind, skip_okf_reserved, output)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if filename == "MEMORY.md" || (skip_okf_reserved && RESERVED_FILENAMES.contains(&filename))
        {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read UTF-8 Markdown {}", path.display()))?;
        let relative_path = path
            .strip_prefix(relative_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        output.push(SourceDocument {
            path,
            relative_path,
            kind,
            raw,
        });
    }
    Ok(())
}

fn write_bundle(
    stage: &Path,
    documents: &[PreparedDocument],
    link_map: &HashMap<String, String>,
    source_name: &str,
) -> anyhow::Result<()> {
    for directory in ["pages", "memories", "journals"] {
        fs::create_dir_all(stage.join(directory))?;
    }
    for document in documents {
        let output = stage.join(&document.destination);
        let body = convert_wikilinks(&document.body, link_map);
        let yaml = serde_yaml_ng::to_string(&document.metadata)?;
        let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
        let content = format!("---\n{}---\n{}", yaml, ensure_leading_newline(&body));
        fs::write(&output, content)
            .with_context(|| format!("write OKF concept {}", output.display()))?;
    }
    write_indexes(stage, documents, source_name)
}

fn write_indexes(
    stage: &Path,
    documents: &[PreparedDocument],
    source_name: &str,
) -> anyhow::Result<()> {
    let mut root = format!(
        "---\nokf_version: \"{OKF_VERSION}\"\n---\n# Makakoo knowledge bundle: {source_name}\n\n"
    );
    for (kind, heading) in [
        (ConceptKind::Page, "Pages"),
        (ConceptKind::Memory, "Memories"),
        (ConceptKind::Journal, "Journals"),
    ] {
        let matching: Vec<_> = documents
            .iter()
            .filter(|document| document.kind == kind)
            .collect();
        if matching.is_empty() {
            continue;
        }
        root.push_str(&format!("## {heading}\n\n"));
        let directory = kind.directory();
        root.push_str(&format!(
            "* [{heading}]({directory}/) - {} concepts\n\n",
            matching.len()
        ));
        let mut index = format!("# {heading}\n\n");
        for document in matching {
            let filename = Path::new(&document.destination)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            index.push_str(&format!(
                "* [{}]({}) - {}\n",
                escape_markdown_label(&document.title),
                filename,
                one_line(&document.description)
            ));
        }
        fs::write(stage.join(directory).join("index.md"), index)?;
    }
    fs::write(stage.join("index.md"), root)?;
    Ok(())
}

fn build_link_map(documents: &[PreparedDocument]) -> HashMap<String, String> {
    let mut output = HashMap::new();
    for document in documents {
        let target = format!("/{}", document.destination);
        for key in &document.lookup_keys {
            output
                .entry(key.to_string())
                .or_insert_with(|| target.clone());
            output
                .entry(key.to_ascii_lowercase())
                .or_insert_with(|| target.clone());
        }
    }
    output
}

fn convert_wikilinks(body: &str, link_map: &HashMap<String, String>) -> String {
    let regex = Regex::new(
        r"(?P<embed>!)?\[\[(?P<target>[^#|\]]+)(?:#(?P<heading>[^|\]]+))?(?:\|(?P<label>[^\]]+))?\]\]",
    )
    .expect("wikilink regex");
    regex
        .replace_all(body, |captures: &Captures<'_>| {
            let target = captures
                .name("target")
                .map(|value| value.as_str().trim())
                .unwrap_or("");
            let label = captures
                .name("label")
                .map(|value| value.as_str().trim())
                .filter(|value| !value.is_empty())
                .unwrap_or(target);
            let mut destination = link_map
                .get(target)
                .or_else(|| link_map.get(&target.to_ascii_lowercase()))
                .cloned()
                .unwrap_or_else(|| format!("/pages/{}.md", slugify(target)));
            if let Some(heading) = captures.name("heading") {
                destination.push('#');
                destination.push_str(&slugify(heading.as_str()));
            }
            format!("[{}]({destination})", escape_markdown_label(label))
        })
        .into_owned()
}

fn parse_optional_frontmatter(raw: &str) -> anyhow::Result<(Mapping, String)> {
    if !raw.starts_with("---") {
        return Ok((Mapping::new(), raw.to_string()));
    }
    let (yaml, body) = split_frontmatter(raw)?;
    let value: Value = serde_yaml_ng::from_str(yaml)?;
    let mapping = value
        .as_mapping()
        .cloned()
        .context("frontmatter must be a YAML mapping")?;
    Ok((mapping, body.to_string()))
}

fn split_frontmatter(raw: &str) -> anyhow::Result<(&str, &str)> {
    let mut offset = 0usize;
    let mut yaml_start = None;
    for (line_number, line) in raw.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if line_number == 0 {
            if trimmed != "---" {
                bail!("frontmatter must start with '---' on its own line");
            }
            yaml_start = Some(line.len());
        } else if trimmed == "---" {
            let start = yaml_start.unwrap_or(0);
            let body_start = offset + line.len();
            return Ok((&raw[start..offset], &raw[body_start..]));
        }
        offset += line.len();
    }
    bail!("frontmatter closing delimiter is missing")
}

fn mapping_to_btree(mapping: Mapping) -> anyhow::Result<BTreeMap<String, Value>> {
    let mut output = BTreeMap::new();
    for (key, value) in mapping {
        let key = key
            .as_str()
            .context("frontmatter keys must be strings")?
            .to_string();
        output.insert(key, value);
    }
    Ok(output)
}

fn extract_logseq_properties(body: &str) -> HashMap<String, String> {
    let mut output = HashMap::new();
    for line in body.lines().take(40) {
        let trimmed = line.trim().trim_start_matches("- ").trim();
        if trimmed.is_empty() {
            if !output.is_empty() {
                break;
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once("::") else {
            if !output.is_empty() {
                break;
            }
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            output.insert(key, value.to_string());
        }
    }
    output
}

fn metadata_string(mapping: &Mapping, key: &str) -> Option<String> {
    mapping
        .get(Value::String(key.to_string()))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn collect_tags(
    metadata: &BTreeMap<String, Value>,
    logseq: &HashMap<String, String>,
) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(value) = metadata.get("tags") {
        match value {
            Value::Sequence(values) => {
                for value in values {
                    if let Some(value) = value.as_str() {
                        push_tag(&mut tags, value);
                    }
                }
            }
            Value::String(value) => {
                for part in value.split([',', ' ']) {
                    push_tag(&mut tags, part);
                }
            }
            _ => {}
        }
    }
    if let Some(value) = logseq.get("tags") {
        for part in value.split([',', ' ']) {
            push_tag(&mut tags, part);
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn nested_metadata_string(
    metadata: &BTreeMap<String, Value>,
    parent: &str,
    key: &str,
) -> Option<String> {
    metadata
        .get(parent)
        .and_then(Value::as_mapping)
        .and_then(|mapping| mapping.get(Value::String(key.to_string())))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn push_tag(tags: &mut Vec<String>, value: &str) {
    let value = value
        .trim()
        .trim_start_matches('#')
        .trim_start_matches("[[")
        .trim_end_matches("]]")
        .trim();
    if !value.is_empty() {
        tags.push(value.to_string());
    }
}

fn is_public(mapping: &Mapping, logseq: &HashMap<String, String>) -> bool {
    metadata_string(mapping, "visibility")
        .or_else(|| logseq.get("visibility").cloned())
        .is_some_and(|value| value.eq_ignore_ascii_case("public"))
}

fn likely_secret(raw: &str) -> Option<&'static str> {
    let upper = raw.to_ascii_uppercase();
    if upper.contains("-----BEGIN PRIVATE KEY-----")
        || upper.contains("-----BEGIN RSA PRIVATE KEY-----")
        || upper.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
    {
        return Some("private-key block");
    }
    let token =
        Regex::new(r"(?i)\b(sk-[a-z0-9_-]{16,}|gh[pousr]_[a-z0-9]{20,}|AKIA[0-9A-Z]{16})\b")
            .expect("secret token regex");
    if token.is_match(raw) {
        return Some("credential-shaped token");
    }
    let assignment = Regex::new(
        r#"(?im)^\s*(password|secret|api[_-]?key|access[_-]?token)\s*(::|:)\s*["']?[^\s<{][^\s]{7,}"#,
    )
    .expect("secret assignment regex");
    if assignment.is_match(raw) {
        return Some("credential assignment");
    }
    None
}

fn first_h1(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn derive_description(body: &str, title: &str) -> String {
    let wikilink =
        Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]").expect("description wikilink regex");
    for line in body.lines() {
        let line = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches('#')
            .trim();
        if line.is_empty() || line == title || line.contains("::") {
            continue;
        }
        let line = wikilink.replace_all(line, "$1");
        return line.chars().take(240).collect();
    }
    format!("Knowledge about {title}.")
}

fn display_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("concept")
        .replace('_', " ")
        .trim()
        .to_string()
}

fn slugify(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            separator = false;
            output.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    let output = output.trim_matches('-');
    if output.is_empty() {
        "concept".to_string()
    } else {
        output.to_string()
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn modified_timestamp(path: &Path) -> String {
    let modified = path
        .metadata()
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    DateTime::<Utc>::from(modified).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(decoded) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(decoded);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn escape_markdown_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace(']', "\\]")
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ensure_leading_newline(value: &str) -> String {
    if value.starts_with('\n') {
        value.to_string()
    } else {
        format!("\n{value}")
    }
}

fn directory_has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().transpose()?.is_some())
}

fn canonical_or_absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    absolute_without_existing(path)
}

fn absolute_without_existing(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalize_path(&absolute))
}

fn canonicalize_existing_ancestor(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Prefix(_) | Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(_) => {
                        resolved = candidate.canonicalize().with_context(|| {
                            format!("canonicalize output ancestor {}", candidate.display())
                        })?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved = candidate;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(normalize_path(&resolved))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn path_entry_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn os_str_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        value
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    }
    #[cfg(not(any(unix, windows)))]
    value.to_string_lossy().as_bytes().to_vec()
}

fn os_str_digest(value: &std::ffi::OsStr) -> String {
    let mut digest = Sha256::new();
    digest.update(os_str_bytes(value));
    format!("{:x}", digest.finalize())
}

fn output_identity(output: &Path) -> anyhow::Result<String> {
    let absolute = canonicalize_existing_ancestor(output)?;
    Ok(os_str_digest(absolute.as_os_str()))
}

fn output_artifact_key(output: &Path) -> String {
    os_str_digest(output.file_name().unwrap_or(output.as_os_str()))
}

fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn staging_path(output: &Path, label: &str) -> PathBuf {
    let parent = parent_or_current(output);
    parent.join(format!(
        ".makakoo-okf-{}-{label}",
        output_artifact_key(output)
    ))
}

fn recovery_marker_path(output: &Path, kind: &str) -> PathBuf {
    staging_path(output, &format!("{kind}.owner"))
}

fn recovery_marker_body(output: &Path, kind: &str) -> anyhow::Result<String> {
    Ok(format!(
        "{RECOVERY_MARKER_VERSION}\nkind={kind}\noutput_sha256={}\n",
        output_identity(output)?
    ))
}

fn lock_export_output(output: &Path) -> anyhow::Result<File> {
    let parent = parent_or_current(output);
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(parent.join(format!(".makakoo-okf-{}.lock", output_artifact_key(output))))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(directory)
        .with_context(|| format!("open directory for sync: {}", directory.display()))?
        .sync_all()
        .with_context(|| format!("sync directory: {}", directory.display()))?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

fn sync_tree(directory: &Path) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            sync_tree(&path)?;
        } else if file_type.is_file() {
            // Windows FlushFileBuffers requires a handle opened for writing.
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?
                .sync_all()
                .with_context(|| format!("sync OKF file: {}", path.display()))?;
        }
    }
    sync_directory(directory)
}

fn tree_digest(directory: &Path) -> anyhow::Result<String> {
    fn update(root: &Path, directory: &Path, digest: &mut Sha256) -> anyhow::Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative = os_str_bytes(relative.as_os_str());
            if file_type.is_dir() {
                digest.update(b"D");
                digest.update((relative.len() as u64).to_le_bytes());
                digest.update(&relative);
                update(root, &path, digest)?;
            } else if file_type.is_file() {
                digest.update(b"F");
                digest.update((relative.len() as u64).to_le_bytes());
                digest.update(&relative);
                let mut file = File::open(&path)?;
                let length = file.metadata()?.len();
                digest.update(length.to_le_bytes());
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    digest.update(&buffer[..read]);
                }
            } else {
                bail!(
                    "refusing non-file entry in owned OKF bundle: {}",
                    path.display()
                );
            }
        }
        Ok(())
    }

    let mut digest = Sha256::new();
    update(directory, directory, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

fn recovery_marker_is_owned(output: &Path, kind: &str) -> anyhow::Result<bool> {
    let marker = recovery_marker_path(output, kind);
    if !path_entry_exists(&marker)? {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(&marker)?;
    if !metadata.file_type().is_file() {
        return Ok(false);
    }
    Ok(fs::read_to_string(marker)? == recovery_marker_body(output, kind)?)
}

fn require_owned_recovery_artifact(output: &Path, kind: &str) -> anyhow::Result<()> {
    if !recovery_marker_is_owned(output, kind)? {
        bail!(
            "refusing unowned OKF recovery artifact {}; move it aside or remove it manually",
            staging_path(output, kind).display()
        );
    }
    let artifact = staging_path(output, kind);
    if path_entry_exists(&artifact)? && !fs::symlink_metadata(&artifact)?.file_type().is_dir() {
        bail!(
            "refusing non-directory OKF recovery artifact {}; move it aside or remove it manually",
            artifact.display()
        );
    }
    Ok(())
}

fn write_recovery_marker(output: &Path, kind: &str) -> anyhow::Result<()> {
    let marker = recovery_marker_path(output, kind);
    if path_entry_exists(&marker)? {
        bail!(
            "OKF recovery marker collision at {}; move it aside or remove it manually",
            marker.display()
        );
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)?;
    file.write_all(recovery_marker_body(output, kind)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    sync_directory(parent_or_current(&marker))
}

fn remove_recovery_marker(output: &Path, kind: &str) -> anyhow::Result<()> {
    let marker = recovery_marker_path(output, kind);
    if !path_entry_exists(&marker)? {
        return Ok(());
    }
    if !recovery_marker_is_owned(output, kind)? {
        bail!(
            "refusing unowned OKF recovery marker {}; move it aside or remove it manually",
            marker.display()
        );
    }
    fs::remove_file(&marker)?;
    sync_directory(parent_or_current(&marker))
}

fn promotion_marker_prefix(output: &Path) -> anyhow::Result<String> {
    Ok(format!(
        "{RECOVERY_MARKER_VERSION}\nkind=promoted\noutput_sha256={}\nsha256=",
        output_identity(output)?
    ))
}

fn promotion_marker_digest(output: &Path) -> anyhow::Result<Option<String>> {
    let marker = recovery_marker_path(output, "promoted");
    if !path_entry_exists(&marker)? {
        return Ok(None);
    }
    if !fs::symlink_metadata(&marker)?.file_type().is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(marker)?;
    let Some(digest) = raw.strip_prefix(&promotion_marker_prefix(output)?) else {
        return Ok(None);
    };
    let digest = digest.trim_end();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(None);
    }
    Ok(Some(digest.to_ascii_lowercase()))
}

fn write_promotion_marker(output: &Path, digest: &str) -> anyhow::Result<()> {
    let marker = recovery_marker_path(output, "promoted");
    if path_entry_exists(&marker)? {
        bail!(
            "OKF promotion marker collision at {}; move it aside or remove it manually",
            marker.display()
        );
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)?;
    file.write_all(promotion_marker_prefix(output)?.as_bytes())?;
    file.write_all(digest.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    sync_directory(parent_or_current(&marker))
}

fn remove_promotion_marker(output: &Path) -> anyhow::Result<()> {
    let marker = recovery_marker_path(output, "promoted");
    if !path_entry_exists(&marker)? {
        return Ok(());
    }
    if promotion_marker_digest(output)?.is_none() {
        bail!(
            "refusing unowned OKF promotion marker {}; move it aside or remove it manually",
            marker.display()
        );
    }
    fs::remove_file(&marker)?;
    sync_directory(parent_or_current(&marker))
}

fn verify_promoted_output(output: &Path) -> anyhow::Result<()> {
    let expected = promotion_marker_digest(output)?
        .context("owned OKF backup exists without a valid promotion marker")?;
    if !path_entry_exists(output)? || !fs::symlink_metadata(output)?.file_type().is_dir() {
        bail!(
            "refusing to discard OKF backup because promoted output is not an owned directory: {}",
            output.display()
        );
    }
    let actual = tree_digest(output)?;
    if actual != expected {
        bail!("refusing to discard OKF backup because output does not match the owned promotion");
    }
    Ok(())
}

fn cleanup_owned_stage(output: &Path) -> anyhow::Result<()> {
    let stage = staging_path(output, "stage");
    require_owned_recovery_artifact(output, "stage")?;
    if path_entry_exists(&stage)? {
        fs::remove_dir_all(&stage)?;
        sync_directory(parent_or_current(&stage))?;
    }
    remove_recovery_marker(output, "stage")
}

fn recover_export_output(output: &Path) -> anyhow::Result<()> {
    let stage = staging_path(output, "stage");
    let backup = staging_path(output, "backup");
    let parent = parent_or_current(output);
    let backup_marker = recovery_marker_path(output, "backup");
    let promotion_marker = recovery_marker_path(output, "promoted");
    if path_entry_exists(&backup)? {
        require_owned_recovery_artifact(output, "backup")?;
        if !path_entry_exists(output)? {
            if path_entry_exists(&promotion_marker)? {
                // Persist rollback intent before restoring the old bundle so a
                // second crash cannot misidentify it as the staged promotion.
                remove_promotion_marker(output)?;
            }
            fs::rename(&backup, output).context("recover interrupted OKF replacement")?;
            sync_directory(parent)?;
        } else {
            verify_promoted_output(output)?;
            // Persist the verified promotion before deleting the known-good fallback.
            sync_directory(parent)?;
            fs::remove_dir_all(&backup)?;
            sync_directory(parent)?;
        }
        remove_recovery_marker(output, "backup")?;
        if path_entry_exists(&promotion_marker)? {
            remove_promotion_marker(output)?;
        }
    } else {
        if path_entry_exists(&backup_marker)? {
            remove_recovery_marker(output, "backup")?;
        }
        if path_entry_exists(&promotion_marker)? {
            if path_entry_exists(&stage)? && recovery_marker_is_owned(output, "stage")? {
                // Rollback completed before its promotion marker was removed.
                // The still-owned stage proves it was never promoted.
                remove_promotion_marker(output)?;
            } else {
                verify_promoted_output(output)?;
                remove_promotion_marker(output)?;
            }
        }
    }

    let stage_marker = recovery_marker_path(output, "stage");
    if path_entry_exists(&stage)? {
        cleanup_owned_stage(output)?;
    } else if path_entry_exists(&stage_marker)? {
        remove_recovery_marker(output, "stage")?;
    }
    if path_entry_exists(output)? {
        sync_directory(parent)?;
    }
    Ok(())
}

fn commit_staged_directory(
    stage: &Path,
    output: &Path,
    force: bool,
    stage_digest: &str,
) -> anyhow::Result<()> {
    let parent = parent_or_current(output);
    fs::create_dir_all(parent)?;
    if !path_entry_exists(output)? {
        write_promotion_marker(output, stage_digest)?;
        if let Err(error) = fs::rename(stage, output) {
            let _ = remove_promotion_marker(output);
            let _ = cleanup_owned_stage(output);
            return Err(error).context("promote staged OKF bundle");
        }
        sync_directory(parent)?;
        verify_promoted_output(output)?;
        remove_recovery_marker(output, "stage")?;
        remove_promotion_marker(output)?;
        return Ok(());
    }
    if directory_has_entries(output)? && !force {
        cleanup_owned_stage(output)?;
        bail!("output directory became non-empty during export");
    }
    let backup = staging_path(output, "backup");
    if path_entry_exists(&backup)?
        || path_entry_exists(&recovery_marker_path(output, "backup"))?
        || path_entry_exists(&recovery_marker_path(output, "promoted"))?
    {
        bail!(
            "OKF backup recovery artifact appeared during export: {}",
            backup.display()
        );
    }
    write_recovery_marker(output, "backup")?;
    if let Err(error) = fs::rename(output, &backup) {
        let _ = remove_recovery_marker(output, "backup");
        return Err(error).context("stage existing OKF bundle as backup");
    }
    sync_directory(parent)?;
    write_promotion_marker(output, stage_digest)?;
    if let Err(error) = fs::rename(stage, output) {
        let promotion_removed = remove_promotion_marker(output).is_ok();
        if promotion_removed && fs::rename(&backup, output).is_ok() {
            let _ = sync_directory(parent);
            let _ = remove_recovery_marker(output, "backup");
        }
        return Err(error).context("promote staged OKF bundle");
    }
    sync_directory(parent)?;
    verify_promoted_output(output)?;
    remove_recovery_marker(output, "stage")?;
    fs::remove_dir_all(&backup)?;
    sync_directory(parent)?;
    remove_recovery_marker(output, "backup")?;
    remove_promotion_marker(output)?;
    Ok(())
}

pub fn validate_bundle(bundle: &Path) -> anyhow::Result<ValidationReport> {
    if !bundle.is_dir() {
        bail!("OKF bundle is not a directory: {}", bundle.display());
    }
    let bundle = bundle.canonicalize()?;
    let mut report = ValidationReport {
        version: OKF_VERSION.to_string(),
        bundle: bundle.to_string_lossy().into_owned(),
        concepts: 0,
        indexes: 0,
        logs: 0,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    let mut files = Vec::new();
    collect_validation_files(&bundle, &bundle, &mut files, &mut report)?;
    files.sort();
    for path in files {
        validate_file(&bundle, &path, &mut report);
    }
    if report.concepts == 0 {
        report.warnings.push(Diagnostic {
            path: ".".to_string(),
            message: "bundle contains no concept documents".to_string(),
        });
    }
    if report.indexes == 0 {
        report.warnings.push(Diagnostic {
            path: ".".to_string(),
            message: "bundle has no index.md; progressive disclosure is unavailable".to_string(),
        });
    }
    Ok(report)
}

fn collect_validation_files(
    bundle: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
    report: &mut ValidationReport,
) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            report.warnings.push(Diagnostic {
                path: relative_display(bundle, &path),
                message: "symlink was not traversed; bundle may not be portable".to_string(),
            });
        } else if file_type.is_dir() {
            collect_validation_files(bundle, &path, output, report)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
            output.push(path);
        }
    }
    Ok(())
}

fn validate_file(bundle: &Path, path: &Path, report: &mut ValidationReport) {
    let relative = relative_display(bundle, path);
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            report.errors.push(Diagnostic {
                path: relative,
                message: format!("not valid UTF-8 Markdown: {error}"),
            });
            return;
        }
    };
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if filename == "index.md" {
        report.indexes += 1;
        validate_index(bundle, path, &content, report);
    } else if filename == "log.md" {
        report.logs += 1;
        validate_log(bundle, path, &content, report);
    } else {
        report.concepts += 1;
        validate_concept(bundle, path, &content, report);
    }
    validate_links(bundle, path, &content, report);
}

fn validate_concept(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    let relative = relative_display(bundle, path);
    let (yaml, _body) = match split_frontmatter(content) {
        Ok(parts) => parts,
        Err(error) => {
            report.errors.push(Diagnostic {
                path: relative,
                message: error.to_string(),
            });
            return;
        }
    };
    let value: Value = match serde_yaml_ng::from_str(yaml) {
        Ok(value) => value,
        Err(error) => {
            report.errors.push(Diagnostic {
                path: relative,
                message: format!("frontmatter is not parseable YAML: {error}"),
            });
            return;
        }
    };
    let Some(mapping) = value.as_mapping() else {
        report.errors.push(Diagnostic {
            path: relative,
            message: "frontmatter must be a YAML mapping".to_string(),
        });
        return;
    };
    if metadata_string(mapping, "type").is_none() {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "frontmatter requires a non-empty string 'type'".to_string(),
        });
    }
    if let Some(timestamp) = metadata_string(mapping, "timestamp") {
        if DateTime::parse_from_rfc3339(&timestamp).is_err() {
            report.warnings.push(Diagnostic {
                path: relative,
                message: "timestamp is not an ISO 8601 datetime".to_string(),
            });
        }
    }
}

fn validate_index(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    let relative = relative_display(bundle, path);
    let is_root = path.parent() == Some(bundle);
    let body = if content.starts_with("---") {
        if !is_root {
            report.errors.push(Diagnostic {
                path: relative.clone(),
                message: "frontmatter is only permitted on the bundle-root index.md".to_string(),
            });
        }
        match split_frontmatter(content) {
            Ok((yaml, body)) => {
                match serde_yaml_ng::from_str::<Value>(yaml) {
                    Ok(value) => {
                        if is_root {
                            let version = value
                                .as_mapping()
                                .and_then(|mapping| metadata_string(mapping, "okf_version"));
                            match version.as_deref() {
                                Some(OKF_VERSION) => {}
                                Some(other) => report.warnings.push(Diagnostic {
                                    path: relative.clone(),
                                    message: format!(
                                        "unsupported okf_version {other:?}; validation continued best-effort"
                                    ),
                                }),
                                None => report.warnings.push(Diagnostic {
                                    path: relative.clone(),
                                    message: "root index does not declare okf_version".to_string(),
                                }),
                            }
                        }
                    }
                    Err(error) => report.errors.push(Diagnostic {
                        path: relative.clone(),
                        message: format!("index frontmatter is invalid YAML: {error}"),
                    }),
                }
                body
            }
            Err(error) => {
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: error.to_string(),
                });
                content
            }
        }
    } else {
        if is_root {
            report.warnings.push(Diagnostic {
                path: relative.clone(),
                message: "root index does not declare okf_version".to_string(),
            });
        }
        content
    };
    let mut in_fence: Option<(char, usize)> = None;
    let mut has_section_heading = false;
    let mut has_grouped_link = false;
    let mut current_section: Option<(usize, usize, usize, bool)> = None;
    let mut current_entry_open = false;
    let mut current_entry_indent: Option<usize> = None;
    let lines: Vec<_> = body.lines().collect();
    let parsed_links = makakoo_core::markdown::links(body);
    let reference_definitions = makakoo_core::markdown::reference_definition_ranges(body);
    let mut skip_setext_underline = false;
    for (line_index, line) in lines.iter().copied().enumerate() {
        if skip_setext_underline {
            skip_setext_underline = false;
            continue;
        }
        let trimmed = line.trim_start();
        let fence_token = markdown_fence_token(line).or_else(|| {
            current_entry_indent.and_then(|indent| markdown_fence_token_in_container(line, indent))
        });
        if let Some((marker, length, trailing)) = fence_token {
            match in_fence {
                Some((open_marker, open_length))
                    if marker == open_marker
                        && length >= open_length
                        && trailing.trim().is_empty() =>
                {
                    in_fence = None;
                }
                None if marker != '`' || !trailing.contains('`') => {
                    in_fence = Some((marker, length));
                }
                _ => {}
            }
            continue;
        }
        if in_fence.is_some() || line.starts_with("    ") {
            continue;
        }
        if trimmed.is_empty() {
            current_entry_open = false;
            continue;
        }
        let line_offset = line.as_ptr() as usize - body.as_ptr() as usize;
        if reference_definitions
            .iter()
            .any(|definition| definition.contains(&line_offset))
        {
            continue;
        }
        let atx_heading = markdown_atx_heading_level(line);
        let setext_heading = atx_heading.is_none().then(|| {
            lines
                .get(line_index + 1)
                .and_then(|underline| markdown_setext_heading_level(line, underline))
        });
        let setext_heading = setext_heading.flatten();
        if setext_heading.is_some() {
            skip_setext_underline = true;
        }
        if let Some(heading_level) = atx_heading.or(setext_heading) {
            if let Some((heading_line, 0, previous_level, title_candidate)) = current_section {
                if !title_candidate || heading_level <= previous_level {
                    report.errors.push(Diagnostic {
                        path: relative.clone(),
                        message: format!(
                            "index.md section at line {heading_line} requires at least one Markdown list link"
                        ),
                    });
                }
            }
            let title_candidate = !has_section_heading && heading_level == 1;
            has_section_heading = true;
            current_section = Some((line_index + 1, 0, heading_level, title_candidate));
            current_entry_open = false;
            current_entry_indent = None;
            continue;
        }
        if let Some((item, item_indent)) = markdown_top_level_list_item(line) {
            current_entry_open = false;
            current_entry_indent = None;
            let Some((_, entries, _, _)) = current_section.as_mut() else {
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!(
                        "index.md line {} has a list entry outside a section heading",
                        line_index + 1
                    ),
                });
                continue;
            };
            let item_offset = item.as_ptr() as usize - body.as_ptr() as usize;
            let valid_target = parsed_links
                .iter()
                .find(|link| link.source_range.start == item_offset)
                .is_some_and(|link| {
                    index_target_is_local_concept_or_directory(bundle, path, &link.destination)
                });
            if valid_target {
                *entries += 1;
                has_grouped_link = true;
                current_entry_open = true;
                current_entry_indent = Some(item_indent);
            } else {
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!(
                        "index.md line {} must link to a local concept or directory",
                        line_index + 1
                    ),
                });
            }
            continue;
        }
        let nested_list = markdown_list_item(trimmed).is_some();
        let indented_continuation = (line.starts_with("  ") || line.starts_with('\t'))
            && current_entry_open
            && !nested_list;
        if indented_continuation {
            continue;
        }
        current_entry_open = false;
        current_entry_indent = None;
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: format!(
                "index.md line {} must be a section heading or Markdown list link",
                line_index + 1
            ),
        });
    }
    if let Some((heading_line, 0, _, _)) = current_section {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: format!(
                "index.md section at line {heading_line} requires at least one Markdown list link"
            ),
        });
    }
    if in_fence.is_some() {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "index.md has an unclosed fenced code block".to_string(),
        });
    }
    if !has_section_heading {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "index.md requires at least one Markdown section heading".to_string(),
        });
    }
    if !has_grouped_link {
        report.errors.push(Diagnostic {
            path: relative,
            message: "index.md requires at least one Markdown list link beneath a section heading"
                .to_string(),
        });
    }
}

fn markdown_fence_token(line: &str) -> Option<(char, usize, &str)> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 || bytes.get(indent) == Some(&b'\t') {
        return None;
    }
    let marker = *bytes.get(indent)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = bytes[indent..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    if length < 3 {
        return None;
    }
    Some((marker as char, length, &line[indent + length..]))
}

fn markdown_fence_token_in_container(
    line: &str,
    content_indent: usize,
) -> Option<(char, usize, &str)> {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    if spaces >= content_indent {
        return markdown_fence_token(&line[content_indent..]);
    }
    line.strip_prefix('\t').and_then(markdown_fence_token)
}

fn markdown_unordered_list_item(line: &str) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    if !matches!(bytes.first(), Some(b'-' | b'*' | b'+')) {
        return None;
    }
    markdown_list_item_after_marker(line, 1)
}

fn markdown_ordered_list_item(line: &str) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
        return None;
    }
    markdown_list_item_after_marker(line, digits + 1)
}

fn markdown_list_item(line: &str) -> Option<(&str, usize)> {
    markdown_unordered_list_item(line).or_else(|| markdown_ordered_list_item(line))
}

fn markdown_list_item_after_marker(line: &str, marker_end: usize) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let whitespace = bytes[marker_end..]
        .iter()
        .take_while(|byte| **byte == b' ' || **byte == b'\t')
        .count();
    if whitespace == 0 {
        return None;
    }
    let start = marker_end + whitespace;
    let indent = bytes[marker_end..start]
        .iter()
        .fold(marker_end, |column, byte| {
            if *byte == b'\t' {
                ((column / 4) + 1) * 4
            } else {
                column + 1
            }
        });
    Some((&line[start..], indent))
}

fn markdown_top_level_list_item(line: &str) -> Option<(&str, usize)> {
    let leading_spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    if leading_spaces > 3 || line.as_bytes().get(leading_spaces) == Some(&b'\t') {
        return None;
    }
    markdown_list_item(&line[leading_spaces..])
        .map(|(item, indent)| (item, leading_spaces + indent))
}

fn markdown_atx_heading(line: &str) -> Option<(usize, &str)> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 || bytes.get(indent) == Some(&b'\t') {
        return None;
    }
    let hashes = bytes[indent..]
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[indent + hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let content = rest.trim();
    let without_hashes = content.trim_end_matches('#');
    let content = if without_hashes.len() < content.len()
        && (without_hashes.is_empty()
            || without_hashes.ends_with(' ')
            || without_hashes.ends_with('\t'))
    {
        without_hashes.trim_end()
    } else {
        content
    };
    Some((hashes, content))
}

fn markdown_atx_heading_level(line: &str) -> Option<usize> {
    markdown_atx_heading(line).and_then(|(level, content)| (!content.is_empty()).then_some(level))
}

fn markdown_setext_heading_level(text: &str, underline: &str) -> Option<usize> {
    if text.trim().is_empty() || text.starts_with("    ") || text.starts_with('\t') {
        return None;
    }
    let bytes = underline.as_bytes();
    let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
    if indent > 3 || bytes.get(indent) == Some(&b'\t') {
        return None;
    }
    let underline = underline[indent..].trim_end();
    let marker = *underline.as_bytes().first()?;
    if (marker != b'=' && marker != b'-')
        || !underline.as_bytes().iter().all(|byte| *byte == marker)
    {
        return None;
    }
    Some(if marker == b'=' { 1 } else { 2 })
}

fn index_target_is_local_concept_or_directory(bundle: &Path, index: &Path, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with('#')
        || target.starts_with("//")
        || target.contains("://")
        || target.starts_with("mailto:")
        || target.starts_with("data:")
    {
        return false;
    }
    let target = percent_decode(target.split(['#', '?']).next().unwrap_or(""));
    if target.is_empty() {
        return false;
    }
    let candidate = if target.starts_with('/') {
        bundle.join(target.trim_start_matches('/'))
    } else {
        index.parent().unwrap_or(bundle).join(&target)
    };
    let candidate = normalize_path(&candidate);
    if !candidate.starts_with(bundle) {
        return false;
    }
    target.ends_with('/')
        || candidate.is_dir()
        || candidate.extension().and_then(|value| value.to_str()) == Some("md")
}

fn validate_log(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    let relative = relative_display(bundle, path);
    if content.starts_with("---") {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "log.md must not contain frontmatter".to_string(),
        });
    }
    let date_value = Regex::new(r"^(\d{4}-\d{2}-\d{2})$").expect("date value regex");
    let mut title_seen = false;
    let mut date_sections = 0usize;
    let mut previous_date: Option<NaiveDate> = None;
    let mut current_date_label: Option<String> = None;
    let mut current_entries = 0usize;
    let mut current_entry_open = false;
    let mut current_entry_indent: Option<usize> = None;
    let mut lazy_continuation_allowed = false;
    let mut in_fence: Option<(char, usize)> = None;

    let lines: Vec<_> = content.lines().collect();
    let mut skip_setext_underline = false;
    for (line_index, raw_line) in lines.iter().copied().enumerate() {
        if skip_setext_underline {
            skip_setext_underline = false;
            continue;
        }
        let line = raw_line.trim_end();
        let fence_token = markdown_fence_token(line).or_else(|| {
            current_entry_indent.and_then(|indent| markdown_fence_token_in_container(line, indent))
        });
        if let Some((marker, length, trailing)) = fence_token {
            match in_fence {
                Some((open_marker, open_length))
                    if marker == open_marker
                        && length >= open_length
                        && trailing.trim().is_empty() =>
                {
                    in_fence = None;
                    continue;
                }
                Some(_) => continue,
                None if current_entry_indent.is_some()
                    && current_date_label.is_some()
                    && (line.starts_with("  ") || line.starts_with('\t'))
                    && (marker != '`' || !trailing.contains('`')) =>
                {
                    in_fence = Some((marker, length));
                    current_entry_open = true;
                    lazy_continuation_allowed = false;
                    continue;
                }
                None => {}
            }
        } else if in_fence.is_some() {
            continue;
        }
        if line.trim().is_empty() {
            current_entry_open = false;
            lazy_continuation_allowed = false;
            continue;
        }
        let atx_heading = markdown_atx_heading(line);
        let atx_title = atx_heading
            .filter(|(level, title)| *level == 1 && !title.is_empty())
            .map(|(_, title)| title);
        let atx_date = atx_heading
            .filter(|(level, _)| *level == 2)
            .and_then(|(_, heading)| date_value.captures(heading))
            .and_then(|captures| captures.get(1))
            .map(|label| label.as_str().to_string());
        let setext_heading = if atx_title.is_none() && atx_date.is_none() {
            lines
                .get(line_index + 1)
                .and_then(|underline| markdown_setext_heading_level(line, underline))
        } else {
            None
        };
        if setext_heading.is_some() {
            skip_setext_underline = true;
        }
        let setext_title = (setext_heading == Some(1)).then_some(line.trim());
        if atx_title.or(setext_title).is_some() {
            if title_seen || date_sections > 0 {
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!(
                        "log.md line {} has an extra H1; exactly one H1 must precede all dates",
                        line_index + 1
                    ),
                });
            } else {
                title_seen = true;
            }
            current_entry_open = false;
            current_entry_indent = None;
            lazy_continuation_allowed = false;
            continue;
        }
        let setext_date = (setext_heading == Some(2))
            .then(|| date_value.captures(line.trim()))
            .flatten()
            .and_then(|captures| captures.get(1))
            .map(|label| label.as_str().to_string());
        if let Some(label) = atx_date.or(setext_date) {
            if let Some(label) = current_date_label.as_deref() {
                if current_entries == 0 {
                    report.errors.push(Diagnostic {
                        path: relative.clone(),
                        message: format!(
                            "log.md date section {label} requires at least one list entry"
                        ),
                    });
                }
            }
            if !title_seen {
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!(
                        "log.md line {} starts a date section before the H1",
                        line_index + 1
                    ),
                });
            }
            match NaiveDate::parse_from_str(&label, "%Y-%m-%d") {
                Ok(date) => {
                    if let Some(previous) = previous_date {
                        if date >= previous {
                            report.errors.push(Diagnostic {
                                path: relative.clone(),
                                message: format!(
                                    "log.md dates must be strictly newest-first: {label} follows {}",
                                    previous.format("%Y-%m-%d")
                                ),
                            });
                        }
                    }
                    previous_date = Some(date);
                }
                Err(_) => report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!("log.md has an invalid ISO date: {label}"),
                }),
            }
            date_sections += 1;
            current_date_label = Some(label);
            current_entries = 0;
            current_entry_open = false;
            current_entry_indent = None;
            lazy_continuation_allowed = false;
            continue;
        }
        if let Some((item, item_indent)) = markdown_top_level_list_item(line) {
            if item.trim().is_empty() {
                current_entry_open = false;
                current_entry_indent = None;
                lazy_continuation_allowed = false;
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!("log.md line {} has an empty list entry", line_index + 1),
                });
                continue;
            }
            if current_date_label.is_some() {
                current_entries += 1;
                current_entry_open = true;
                current_entry_indent = Some(item_indent);
                lazy_continuation_allowed = true;
            } else {
                current_entry_open = false;
                current_entry_indent = None;
                lazy_continuation_allowed = false;
                report.errors.push(Diagnostic {
                    path: relative.clone(),
                    message: format!(
                        "log.md line {} has a list entry outside a date section",
                        line_index + 1
                    ),
                });
            }
            continue;
        }
        let indented_continuation = (line.starts_with("  ") || line.starts_with('\t'))
            && current_entry_open
            && current_date_label.is_some();
        let nested_list = markdown_list_item(line.trim_start()).is_some();
        if indented_continuation && !nested_list {
            lazy_continuation_allowed = true;
            continue;
        }
        let lazy_continuation = lazy_continuation_allowed
            && current_entry_open
            && current_date_label.is_some()
            && !nested_list
            && !line.starts_with('#');
        if lazy_continuation {
            continue;
        }
        current_entry_open = false;
        current_entry_indent = None;
        lazy_continuation_allowed = false;
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: format!(
                "log.md line {} must be an H1, ISO-date H2, or flat list entry",
                line_index + 1
            ),
        });
    }

    if let Some(label) = current_date_label.as_deref() {
        if current_entries == 0 {
            report.errors.push(Diagnostic {
                path: relative.clone(),
                message: format!("log.md date section {label} requires at least one list entry"),
            });
        }
    }
    if in_fence.is_some() {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "log.md has an unclosed fenced code block".to_string(),
        });
    }
    if !title_seen || date_sections == 0 {
        report.errors.push(Diagnostic {
            path: relative,
            message: "log.md requires an H1 and ISO-date H2 entries".to_string(),
        });
    }
}

fn validate_links(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    for target in markdown_link_destinations(content) {
        let target = target.as_str();
        if target.is_empty()
            || target.starts_with('#')
            || target.starts_with("//")
            || target.contains("://")
            || target.starts_with("mailto:")
            || target.starts_with("data:")
        {
            continue;
        }
        let target = percent_decode(target.split(['#', '?']).next().unwrap_or(""));
        let candidate = if target.starts_with('/') {
            bundle.join(target.trim_start_matches('/'))
        } else {
            path.parent().unwrap_or(bundle).join(&target)
        };
        let candidate = normalize_path(&candidate);
        if !candidate.starts_with(bundle) {
            report.warnings.push(Diagnostic {
                path: relative_display(bundle, path),
                message: format!("link target is outside the bundle: {target}"),
            });
            continue;
        }
        let resolved = if candidate.is_dir() {
            candidate.join("index.md")
        } else {
            candidate
        };
        if !resolved.exists() {
            report.warnings.push(Diagnostic {
                path: relative_display(bundle, path),
                message: format!("broken internal link: {target}"),
            });
        }
    }
}

fn markdown_link_destinations(content: &str) -> Vec<String> {
    let markdown = if content.starts_with("---") {
        split_frontmatter(content)
            .map(|(_, body)| body)
            .unwrap_or(content)
    } else {
        content
    };
    makakoo_core::markdown::link_destinations(markdown)
}

fn relative_display(bundle: &Path, path: &Path) -> String {
    path.strip_prefix(bundle)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn options(home: &Path, output: &Path) -> ExportOptions {
        ExportOptions {
            home: home.to_path_buf(),
            source_name: "default".to_string(),
            source_type: "logseq".to_string(),
            source_root: home.join("data/Brain"),
            output: output.to_path_buf(),
            include_journals: false,
            include_auto_memory: true,
            public_only: false,
            force: false,
        }
    }

    #[test]
    fn bare_relative_export_paths_use_current_directory_as_parent() {
        let output = Path::new("bundle");

        assert_eq!(parent_or_current(output), Path::new("."));
        assert_eq!(staging_path(output, "stage").parent(), Some(Path::new(".")));
    }

    #[test]
    fn export_pages_and_memory_converts_wikilinks_and_validates() {
        let dir = tempdir().unwrap();
        let brain = dir.path().join("data/Brain");
        write(
            &brain.join("pages/Makakoo OS.md"),
            "type:: project\ntags:: #makakoo #ai\n\n- [[Harvey|Harvey persona]] runs the platform.",
        );
        write(
            &brain.join("pages/Harvey.md"),
            "# Harvey\n\nPrimary assistant persona.",
        );
        write(
            &brain.join("journals/2026_07_14.md"),
            "- private journal should stay out",
        );
        write(
            &dir.path().join("data/auto-memory/project_x.md"),
            "---\ntype: project\ndescription: Durable project memory.\n---\n# Project X\n",
        );
        let output = dir.path().join("bundle");
        let report = export_bundle(&options(dir.path(), &output)).unwrap();
        assert_eq!(report.pages, 2);
        assert_eq!(report.memories, 1);
        assert_eq!(report.journals, 0);
        let exported = fs::read_to_string(output.join("pages/makakoo-os.md")).unwrap();
        assert!(exported.contains("type: project"));
        assert!(exported.contains("[Harvey persona](/pages/harvey.md)"));
        assert!(!output.join("journals/2026-07-14.md").exists());
        let validation = validate_bundle(&output).unwrap();
        assert!(validation.conformant(), "{:?}", validation.errors);
        assert_eq!(validation.concepts, 3);
        assert!(!recovery_marker_path(&output, "stage").exists());
        assert!(!recovery_marker_path(&output, "backup").exists());
        assert!(!recovery_marker_path(&output, "promoted").exists());
    }

    #[test]
    fn public_export_requires_marker_and_refuses_secret() {
        let dir = tempdir().unwrap();
        let brain = dir.path().join("data/Brain/pages");
        write(&brain.join("Private.md"), "# Private\n\nNot shared.");
        write(
            &brain.join("Public.md"),
            "---\nvisibility: public\n---\n# Public\n\nSafe content.",
        );
        let output = dir.path().join("bundle");
        let mut opts = options(dir.path(), &output);
        opts.include_auto_memory = false;
        opts.public_only = true;
        let report = export_bundle(&opts).unwrap();
        assert_eq!(report.concepts, 1);
        assert_eq!(report.skipped_private, 1);

        write(
            &brain.join("Leaky.md"),
            "---\nvisibility: public\n---\n# Leaky\n\n-----BEGIN PRIVATE KEY-----",
        );
        opts.force = true;
        let error = export_bundle(&opts).unwrap_err().to_string();
        assert!(error.contains("public export refused"));
    }

    #[test]
    fn export_refuses_nonempty_destination_without_force() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("data/Brain/pages/A.md"),
            "# A\n\nKnowledge body.",
        );
        let output = dir.path().join("bundle");
        write(&output.join("keep.txt"), "do not replace");
        let error = export_bundle(&options(dir.path(), &output))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not empty"));
        assert_eq!(
            fs::read_to_string(output.join("keep.txt")).unwrap(),
            "do not replace"
        );
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_output_inside_source_through_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let brain = dir.path().join("data/Brain");
        write(&brain.join("pages/A.md"), "# A\n\nKnowledge body.");
        let alias = dir.path().join("alias");
        symlink(brain.join("pages"), &alias).unwrap();
        let output = alias.join("export");
        let mut opts = options(dir.path(), &output);
        opts.force = true;

        let error = export_bundle(&opts).unwrap_err().to_string();

        assert!(error.contains("output cannot overlap source root"));
        assert!(!brain.join("pages/export").exists());
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_symlink_then_parent_overlap_bypass() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let brain = dir.path().join("data/Brain");
        write(&brain.join("pages/A.md"), "# A\n\nKnowledge body.");
        fs::create_dir_all(brain.join("subdir")).unwrap();
        let alias = dir.path().join("alias");
        symlink(brain.join("subdir"), &alias).unwrap();
        let output = alias.join("../pages");
        let mut opts = options(dir.path(), &output);
        opts.include_auto_memory = false;
        opts.force = true;

        let error = export_bundle(&opts).unwrap_err().to_string();

        assert!(error.contains("output cannot overlap source root"));
        assert!(brain.join("pages/A.md").exists());
    }

    #[test]
    fn export_rejects_output_that_contains_source_root() {
        let dir = tempdir().unwrap();
        let brain = dir.path().join("data/Brain");
        write(&brain.join("pages/A.md"), "# A\n\nKnowledge body.");
        let output = dir.path().join("data");
        let mut opts = options(dir.path(), &output);
        opts.include_auto_memory = false;
        opts.force = true;

        let error = export_bundle(&opts).unwrap_err().to_string();

        assert!(error.contains("output cannot overlap source root"));
        assert!(brain.join("pages/A.md").exists());
    }

    #[test]
    fn export_recovers_interrupted_force_backup_before_retrying() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("data/Brain/pages/A.md"),
            "# A\n\nKnowledge body.",
        );
        let output = dir.path().join("bundle");
        let backup = staging_path(&output, "backup");
        write(&backup.join("previous.md"), "previous bundle");
        write_recovery_marker(&output, "backup").unwrap();
        let error = export_bundle(&options(dir.path(), &output))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not empty"));
        assert_eq!(
            fs::read_to_string(output.join("previous.md")).unwrap(),
            "previous bundle"
        );
        assert!(!backup.exists());
        assert!(!recovery_marker_path(&output, "backup").exists());
    }

    #[test]
    fn export_preserves_unowned_recovery_directory_collision() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("data/Brain/pages/A.md"),
            "# A\n\nKnowledge body.",
        );
        let output = dir.path().join("bundle");
        let stage = staging_path(&output, "stage");
        write(&stage.join("unrelated.txt"), "must survive");

        let error = export_bundle(&options(dir.path(), &output))
            .unwrap_err()
            .to_string();

        assert!(error.contains("unowned OKF recovery artifact"));
        assert_eq!(
            fs::read_to_string(stage.join("unrelated.txt")).unwrap(),
            "must survive"
        );
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_identity_preserves_non_utf8_output_names() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let dir = tempdir().unwrap();
        let first = dir.path().join(OsString::from_vec(vec![
            b'b', b'u', b'n', b'd', b'l', b'e', 0x80,
        ]));
        let second = dir.path().join(OsString::from_vec(vec![
            b'b', b'u', b'n', b'd', b'l', b'e', 0x81,
        ]));

        assert_ne!(
            staging_path(&first, "stage"),
            staging_path(&second, "stage")
        );
        assert_ne!(
            recovery_marker_body(&first, "stage").unwrap(),
            recovery_marker_body(&second, "stage").unwrap()
        );
        assert_ne!(
            promotion_marker_prefix(&first).unwrap(),
            promotion_marker_prefix(&second).unwrap()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tree_digest_preserves_non_utf8_relative_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let dir = tempdir().unwrap();
        let first = dir.path().join(OsString::from_vec(vec![
            b'c', b'o', b'n', b'c', b'e', b'p', b't', 0x80,
        ]));
        let second = dir.path().join(OsString::from_vec(vec![
            b'c', b'o', b'n', b'c', b'e', b'p', b't', 0x81,
        ]));
        fs::write(&first, "same content").unwrap();
        let first_digest = tree_digest(dir.path()).unwrap();
        fs::rename(&first, &second).unwrap();
        let second_digest = tree_digest(dir.path()).unwrap();

        assert_ne!(first_digest, second_digest);
    }

    #[test]
    fn recovery_preserves_backup_until_output_matches_owned_promotion() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("bundle");
        let backup = staging_path(&output, "backup");
        write(&output.join("new.md"), "new bundle");
        write(&backup.join("old.md"), "old bundle");
        write_recovery_marker(&output, "backup").unwrap();

        let error = recover_export_output(&output).unwrap_err().to_string();
        assert!(error.contains("valid promotion marker"));
        assert_eq!(
            fs::read_to_string(backup.join("old.md")).unwrap(),
            "old bundle"
        );
        assert_eq!(
            fs::read_to_string(output.join("new.md")).unwrap(),
            "new bundle"
        );

        let digest = tree_digest(&output).unwrap();
        write_promotion_marker(&output, &digest).unwrap();
        recover_export_output(&output).unwrap();
        assert!(!backup.exists());
        assert!(!recovery_marker_path(&output, "backup").exists());
        assert!(!recovery_marker_path(&output, "promoted").exists());
    }

    #[test]
    fn initial_promotion_verifies_staged_digest() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("bundle");
        let stage = staging_path(&output, "stage");
        write(&stage.join("concept.md"), "expected content");
        write_recovery_marker(&output, "stage").unwrap();
        let digest = tree_digest(&stage).unwrap();
        write(&stage.join("concept.md"), "tampered after digest");

        let error = commit_staged_directory(&stage, &output, false, &digest)
            .unwrap_err()
            .to_string();

        assert!(error.contains("does not match the owned promotion"));
        assert!(output.exists());
        assert!(recovery_marker_path(&output, "stage").exists());
        assert!(recovery_marker_path(&output, "promoted").exists());
    }

    #[test]
    fn recovery_restores_backup_when_promotion_marker_precedes_rename() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("bundle");
        let backup = staging_path(&output, "backup");
        let stage = staging_path(&output, "stage");
        write(&backup.join("old.md"), "old bundle");
        write(&stage.join("new.md"), "new bundle");
        write_recovery_marker(&output, "backup").unwrap();
        write_recovery_marker(&output, "stage").unwrap();
        write_promotion_marker(&output, &tree_digest(&stage).unwrap()).unwrap();

        recover_export_output(&output).unwrap();

        assert_eq!(
            fs::read_to_string(output.join("old.md")).unwrap(),
            "old bundle"
        );
        assert!(!stage.exists());
        assert!(!recovery_marker_path(&output, "backup").exists());
        assert!(!recovery_marker_path(&output, "stage").exists());
        assert!(!recovery_marker_path(&output, "promoted").exists());
    }

    #[test]
    fn recovery_finishes_rollback_after_backup_was_already_restored() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("bundle");
        let stage = staging_path(&output, "stage");
        write(&output.join("old.md"), "restored old bundle");
        write(&stage.join("new.md"), "unpromoted new bundle");
        write_recovery_marker(&output, "stage").unwrap();
        write_promotion_marker(&output, &tree_digest(&stage).unwrap()).unwrap();

        recover_export_output(&output).unwrap();

        assert_eq!(
            fs::read_to_string(output.join("old.md")).unwrap(),
            "restored old bundle"
        );
        assert!(!stage.exists());
        assert!(!recovery_marker_path(&output, "stage").exists());
        assert!(!recovery_marker_path(&output, "promoted").exists());
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_broken_recovery_symlink_without_dropping_marker() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        write(
            &dir.path().join("data/Brain/pages/A.md"),
            "# A\n\nKnowledge body.",
        );
        let output = dir.path().join("bundle");
        let stage = staging_path(&output, "stage");
        symlink(dir.path().join("missing-stage-target"), &stage).unwrap();
        write_recovery_marker(&output, "stage").unwrap();

        let error = export_bundle(&options(dir.path(), &output))
            .unwrap_err()
            .to_string();

        assert!(error.contains("non-directory OKF recovery artifact"));
        assert!(fs::symlink_metadata(&stage)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(recovery_marker_path(&output, "stage").exists());
        assert!(!output.exists());
    }

    #[test]
    fn validation_treats_broken_links_as_warnings_and_missing_type_as_error() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n\n* [Good](good.md)\n",
        );
        write(
            &dir.path().join("good.md"),
            "---\ntype: Topic\n---\n# Good\n\n[Future](missing.md)",
        );
        let warning_report = validate_bundle(dir.path()).unwrap();
        assert!(warning_report.conformant());
        assert_eq!(warning_report.warnings.len(), 1);
        write(&dir.path().join("bad.md"), "---\ntitle: Bad\n---\n# Bad\n");
        let error_report = validate_bundle(dir.path()).unwrap();
        assert!(!error_report.conformant());
        assert!(error_report
            .errors
            .iter()
            .any(|error| error.message.contains("type")));
    }

    #[test]
    fn validation_ignores_link_syntax_inside_code() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n\n`[inline](missing-inline.md)`\n\n    [indented](missing-indented.md)\n\n```markdown\n[fenced](missing-fenced.md)\n```\n\n<pre>\n[html](missing-html.md)\n</pre>\n\n[Real][real]\n\n[real]: real.md\n",
        );
        write(
            &dir.path().join("real.md"),
            "---\ntype: Topic\n---\n# Real\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
        assert!(!report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("missing-inline")
                || warning.message.contains("missing-indented")
                || warning.message.contains("missing-fenced")
                || warning.message.contains("missing-html")));
    }

    #[test]
    fn validation_accepts_reference_links_in_index_entries() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Concepts\n\n* [Concept][concept]\n\n[concept]: concept.md \"Concept title\"\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn validation_accepts_ordered_index_and_log_entries() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Concepts\n\n1. [Concept](concept.md)\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &dir.path().join("log.md"),
            "# Directory Update Log\n\n## 2026-07-14\n1) Added concept\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn validation_best_effort_version_and_escape_are_warnings() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.2\"\n---\n# Bundle\n\n* [Concept](concept.md)\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n\n[Outside](../outside.md)",
        );
        let report = validate_bundle(dir.path()).unwrap();
        assert!(report.conformant());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("unsupported okf_version")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("outside the bundle")));
    }

    #[test]
    fn validation_allows_empty_bundle_with_warnings() {
        let dir = tempdir().unwrap();
        let report = validate_bundle(dir.path()).unwrap();
        assert!(report.conformant());
        assert_eq!(report.concepts, 0);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("no concept documents")));
    }

    #[test]
    fn validation_requires_index_list_link() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(dir.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("Markdown list link")));
    }

    #[test]
    fn validation_index_requires_local_concept_shape_but_tolerates_missing_target() {
        let invalid = tempdir().unwrap();
        write(
            &invalid.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n\n* ![Logo](logo.png)\n* [Image](image.png)\n* [Site](https://example.com)\n",
        );
        write(
            &invalid.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(invalid.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("Markdown list link")));

        let broken = tempdir().unwrap();
        write(
            &broken.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n\n* [Future](future.md)\n",
        );
        write(
            &broken.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(broken.path()).unwrap();
        assert!(report.conformant(), "{:?}", report.errors);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("broken internal link")));
    }

    #[test]
    fn validation_index_ignores_orphan_and_code_block_links() {
        let orphan = tempdir().unwrap();
        write(
            &orphan.path().join("index.md"),
            "* [Orphan](concept.md)\n\n# Section without entries\n",
        );
        write(
            &orphan.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(orphan.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("beneath a section heading")));

        let fenced = tempdir().unwrap();
        write(
            &fenced.path().join("index.md"),
            "```markdown\n    ```\n# Fake section\n* [Fake](concept.md)\n```not-a-close\n# Still fake\n* [Still fake](concept.md)\n```\n",
        );
        write(
            &fenced.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(fenced.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("section heading")));

        let list_fence = tempdir().unwrap();
        write(
            &list_fence.path().join("index.md"),
            "# Concepts\n\n* [Concept](concept.md)\n\n    ```markdown\n    # embedded\n",
        );
        write(
            &list_fence.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        let report = validate_bundle(list_fence.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("unclosed fenced code block")));
    }

    #[test]
    fn validation_index_accepts_balanced_parentheses_and_link_titles() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "## APIs\n\n* [Legacy API](api_(legacy).md \"Legacy title\") - grouped concept\n* [John's notes](john's-notes.md) - apostrophe in destination\n",
        );
        write(
            &dir.path().join("api_(legacy).md"),
            "---\ntype: API\n---\n# Legacy API\n",
        );
        write(
            &dir.path().join("john's-notes.md"),
            "---\ntype: Note\n---\n# John's notes\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
        assert!(!report
            .warnings
            .iter()
            .any(|warning| warning.message.contains("broken internal link")));
    }

    #[test]
    fn validation_accepts_setext_index_and_log_headings() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "Bundle\n======\n\nConcepts\n--------\n\n*\t[Concept](concept.md) - grouped concept\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &dir.path().join("log.md"),
            "Directory Update Log\n====================\n\n2026-07-14\n----------\n*\tAdded concept\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
    }

    #[test]
    fn validation_accepts_indented_atx_log_headings_with_closing_hashes() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &dir.path().join("log.md"),
            " # Directory Update Log #\n\n  ## 2026-07-14 ##\n   *\tAdded concept\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);
    }

    #[test]
    fn validation_rejects_four_space_indented_log_pseudo_entry() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &dir.path().join("log.md"),
            "# Directory Update Log\n\n## 2026-07-14\n    - This is an indented code block, not a list entry\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("requires at least one list entry")));
    }

    #[test]
    fn validation_log_ignores_structure_inside_entry_fences() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &dir.path().join("log.md"),
            "# Log\n\n## 2026-07-14\n- Added fenced example\n\n    ```markdown\n    ## 1900-01-01\n    - not another log entry\n    ```\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(report.conformant(), "{:?}", report.errors);

        write(
            &dir.path().join("log.md"),
            "# Log\n\n## 2026-07-14\n- Added unclosed example\n\n    ```markdown\n    # embedded\n",
        );
        let report = validate_bundle(dir.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("unclosed fenced code block")));
    }

    #[test]
    fn validation_index_checks_every_section_and_entry() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "* [Orphan](concept.md)\n\n# Valid\n\n* [Concept](concept.md)\n  continued description\narbitrary prose\n\n# Empty\n\n# Invalid\n\n* [Site](https://example.com)\n",
        );
        write(
            &dir.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );

        let report = validate_bundle(dir.path()).unwrap();

        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("outside a section heading")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("must be a section heading")));
        assert!(report.errors.iter().any(|error| error
            .message
            .contains("requires at least one Markdown list link")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("local concept or directory")));
    }

    #[test]
    fn uppercase_reserved_names_are_concepts() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n\n* [Upper](upper/INDEX.md)\n",
        );
        write(&dir.path().join("upper/INDEX.md"), "# Missing type\n");
        let report = validate_bundle(dir.path()).unwrap();
        assert_eq!(report.indexes, 1);
        assert_eq!(report.concepts, 1);
        assert!(report
            .errors
            .iter()
            .any(|error| error.path == "upper/INDEX.md" && error.message.contains("frontmatter")));
    }

    #[test]
    fn okf_collection_skips_only_exact_reserved_names() {
        let dir = tempdir().unwrap();
        write(&dir.path().join("index.md"), "# Index\n");
        write(&dir.path().join("log.md"), "# Log\n");
        write(&dir.path().join("concept.md"), "# Concept\n");
        write(&dir.path().join("upper-index/INDEX.md"), "# Upper index\n");
        write(&dir.path().join("upper-log/LOG.md"), "# Upper log\n");
        let mut documents = Vec::new();
        collect_markdown(
            dir.path(),
            dir.path(),
            ConceptKind::Page,
            true,
            &mut documents,
        )
        .unwrap();
        let paths: Vec<_> = documents
            .iter()
            .map(|document| document.relative_path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["concept.md", "upper-index/INDEX.md", "upper-log/LOG.md"]
        );
    }

    #[test]
    fn validation_enforces_log_dates_order_entries_and_structure() {
        let valid = tempdir().unwrap();
        write(
            &valid.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &valid.path().join("log.md"),
            "# Log\n\n## 2026-07-14\n- Created bundle with a detailed description\nthat continues lazily on the next line.\n\n## 2026-07-13\n* Added concept\n",
        );
        assert!(validate_bundle(valid.path()).unwrap().conformant());

        let invalid = tempdir().unwrap();
        write(
            &invalid.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &invalid.path().join("log.md"),
            "# Log\n\n## 2026-02-30\n- Impossible date\n\n## 2026-07-14\n- Older heading\n\n## 2026-07-15\n\n## 2026-07-13\nprose is not a list\n- Valid entry\n",
        );
        let report = validate_bundle(invalid.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("invalid ISO date")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("newest-first")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("requires at least one list entry")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("flat list entry")));

        let detached = tempdir().unwrap();
        write(
            &detached.path().join("concept.md"),
            "---\ntype: Topic\n---\n# Concept\n",
        );
        write(
            &detached.path().join("log.md"),
            "# Log\n\n## 2026-07-14\n- Valid entry\n\n  detached indented prose\n",
        );
        let report = validate_bundle(detached.path()).unwrap();
        assert!(!report.conformant());
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("flat list entry")));
    }

    #[test]
    fn export_is_deterministic_except_for_source_mtime() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("data/Brain/pages/A.md"),
            "---\ntimestamp: 2026-07-14T00:00:00Z\n---\n# A\n\nStable body.",
        );
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        export_bundle(&options(dir.path(), &first)).unwrap();
        export_bundle(&options(dir.path(), &second)).unwrap();
        assert_eq!(
            fs::read_to_string(first.join("pages/a.md")).unwrap(),
            fs::read_to_string(second.join("pages/a.md")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(first.join("index.md")).unwrap(),
            fs::read_to_string(second.join("index.md")).unwrap()
        );
    }
}
