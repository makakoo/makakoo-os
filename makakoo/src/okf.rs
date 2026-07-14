//! Open Knowledge Format v0.1 import/export boundary.
//!
//! Makakoo's Logseq Brain remains canonical. This module only produces and
//! validates portable OKF bundles, with no network or publishing behavior.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context};
use chrono::{DateTime, SecondsFormat, Utc};
use regex::{Captures, Regex};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};

const OKF_VERSION: &str = "0.1";
const RESERVED_FILENAMES: &[&str] = &["index.md", "log.md"];

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
    if stage.exists() {
        fs::remove_dir_all(&stage)
            .with_context(|| format!("remove stale staging dir {}", stage.display()))?;
    }
    fs::create_dir_all(&stage)
        .with_context(|| format!("create staging dir {}", stage.display()))?;

    let build_result = write_bundle(&stage, &prepared, &link_map, &options.source_name);
    if let Err(error) = build_result {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    commit_staged_directory(&stage, &options.output, options.force)?;

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
    if options.output.exists() && options.output.is_file() {
        bail!("output is a file: {}", options.output.display());
    }
    if options.output.exists() && directory_has_entries(&options.output)? && !options.force {
        bail!(
            "output directory is not empty: {} (pass --force to replace it)",
            options.output.display()
        );
    }
    let source = canonical_or_absolute(&options.source_root)?;
    let output = absolute_without_existing(&options.output)?;
    if output.starts_with(&source) {
        bail!("output cannot be inside source root: {}", output.display());
    }
    if options.include_auto_memory {
        let auto_memory = canonical_or_absolute(&options.home.join("data/auto-memory"))?;
        if output.starts_with(&auto_memory) {
            bail!(
                "output cannot be inside auto-memory source: {}",
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
        if filename == "MEMORY.md"
            || (skip_okf_reserved
                && RESERVED_FILENAMES
                    .iter()
                    .any(|reserved| filename.eq_ignore_ascii_case(reserved)))
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
        root.push_str(&format!("# {heading}\n\n"));
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
        .get(&Value::String(key.to_string()))
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
        .and_then(|mapping| mapping.get(&Value::String(key.to_string())))
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
    for line in body.lines() {
        let line = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches('#')
            .trim();
        if line.is_empty() || line == title || line.contains("::") {
            continue;
        }
        let line = Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]")
            .expect("description wikilink regex")
            .replace_all(line, "$1");
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

fn staging_path(output: &Path, label: &str) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("bundle");
    parent.join(format!(".{name}.okf-{label}-{}", std::process::id()))
}

fn commit_staged_directory(stage: &Path, output: &Path, force: bool) -> anyhow::Result<()> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    if !output.exists() {
        fs::rename(stage, output)?;
        return Ok(());
    }
    if directory_has_entries(output)? && !force {
        let _ = fs::remove_dir_all(stage);
        bail!("output directory became non-empty during export");
    }
    let backup = staging_path(output, "backup");
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::rename(output, &backup)?;
    if let Err(error) = fs::rename(stage, output) {
        let _ = fs::rename(&backup, output);
        return Err(error).context("promote staged OKF bundle");
    }
    fs::remove_dir_all(&backup)?;
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
        report.errors.push(Diagnostic {
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
            report.errors.push(Diagnostic {
                path: relative_display(bundle, &path),
                message: "symlinks are not allowed in portable OKF bundles".to_string(),
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
    if filename.eq_ignore_ascii_case("index.md") {
        report.indexes += 1;
        validate_index(bundle, path, &content, report);
    } else if filename.eq_ignore_ascii_case("log.md") {
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
                                Some(other) => report.errors.push(Diagnostic {
                                    path: relative.clone(),
                                    message: format!("unsupported okf_version {other:?}"),
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
    if !body.lines().any(|line| line.starts_with("# ")) {
        report.errors.push(Diagnostic {
            path: relative,
            message: "index.md requires at least one '# ' section heading".to_string(),
        });
    }
}

fn validate_log(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    let relative = relative_display(bundle, path);
    if content.starts_with("---") {
        report.errors.push(Diagnostic {
            path: relative.clone(),
            message: "log.md must not contain frontmatter".to_string(),
        });
    }
    let date_heading = Regex::new(r"(?m)^## \d{4}-\d{2}-\d{2}\s*$").expect("date regex");
    if !content.lines().any(|line| line.starts_with("# ")) || !date_heading.is_match(content) {
        report.errors.push(Diagnostic {
            path: relative,
            message: "log.md requires an H1 and ISO-date '## YYYY-MM-DD' entries".to_string(),
        });
    }
}

fn validate_links(bundle: &Path, path: &Path, content: &str, report: &mut ValidationReport) {
    let regex = Regex::new(r"!?\[[^\]]*\]\(([^)]+)\)").expect("markdown link regex");
    for captures in regex.captures_iter(content) {
        let mut target = captures
            .get(1)
            .map(|value| value.as_str().trim())
            .unwrap_or("");
        if target.starts_with('<') && target.ends_with('>') {
            target = &target[1..target.len() - 1];
        } else if let Some((path, _title)) = target.split_once(char::is_whitespace) {
            target = path;
        }
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
            report.errors.push(Diagnostic {
                path: relative_display(bundle, path),
                message: format!("internal link escapes bundle root: {target}"),
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

    #[test]
    fn validation_treats_broken_links_as_warnings_and_missing_type_as_error() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("index.md"),
            "---\nokf_version: \"0.1\"\n---\n# Bundle\n",
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
