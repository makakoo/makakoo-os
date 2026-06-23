//! SkillSpector macOS security gate bridge.
//!
//! Handles bootstrapping NVIDIA SkillSpector under $MAKAKOO_HOME/state/skillspector-venv/,
//! scanning targets, parsing findings, and enforcing risk policy.

use anyhow::{anyhow, Context};
use chrono::Utc;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::platform::makakoo_home;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillspectorConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_mode")]
    pub mode: String, // static | llm
    #[serde(default = "default_block_on")]
    pub block_on: String, // off | critical | high | medium
    #[serde(default = "default_allow_override")]
    pub allow_override: bool,
    #[serde(default = "default_pinned_git")]
    pub pinned_git: String,
    #[serde(default = "default_pinned_ref")]
    pub pinned_ref: String,
    #[serde(default = "default_report_dir")]
    pub report_dir: String,
}

fn default_enabled() -> bool {
    true
}
fn default_mode() -> String {
    "static".to_string()
}
fn default_block_on() -> String {
    "high".to_string()
}
fn default_allow_override() -> bool {
    true
}
fn default_pinned_git() -> String {
    "https://github.com/NVIDIA/SkillSpector".to_string()
}
fn default_pinned_ref() -> String {
    "939da7d41eed4282e4d8217fe2254c69f690027e".to_string()
}
fn default_report_dir() -> String {
    "$MAKAKOO_HOME/data/reports/skillspector".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillSecurityConfig {
    #[serde(default)]
    pub skillspector: SkillspectorConfig,
}

impl Default for SkillspectorConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            mode: default_mode(),
            block_on: default_block_on(),
            allow_override: default_allow_override(),
            pinned_git: default_pinned_git(),
            pinned_ref: default_pinned_ref(),
            report_dir: default_report_dir(),
        }
    }
}

impl SkillSecurityConfig {
    pub fn load_from(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        toml::from_str::<Self>(&raw).unwrap_or_default()
    }

    pub fn load() -> Self {
        let path = makakoo_home().join("config").join("skill_security.toml");
        Self::load_from(&path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskSeverity {
    Off,
    Low,
    Medium,
    High,
    Critical,
}

impl RiskSeverity {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "OFF" => RiskSeverity::Off,
            "LOW" | "SAFE" => RiskSeverity::Low,
            "MEDIUM" | "CAUTION" => RiskSeverity::Medium,
            "HIGH" => RiskSeverity::High,
            "CRITICAL" => RiskSeverity::Critical,
            _ => RiskSeverity::Low,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillspectorLocation {
    pub file: String,
    pub line: Option<u32>,
}

impl<'de> Deserialize<'de> for SkillspectorLocation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LocationVisitor;

        impl<'de> Visitor<'de> for LocationVisitor {
            type Value = SkillspectorLocation;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("string or map with file and line")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if let Some(pos) = value.find(':') {
                    let file = value[..pos].to_string();
                    let line = value[pos + 1..].parse::<u32>().ok();
                    Ok(SkillspectorLocation { file, line })
                } else {
                    Ok(SkillspectorLocation {
                        file: value.to_string(),
                        line: None,
                    })
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut file = None;
                let mut line = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "file" | "path" => {
                            file = Some(map.next_value::<String>()?);
                        }
                        "line" | "line_number" | "start_line" => {
                            let val = map.next_value::<serde_json::Value>()?;
                            if let Some(n) = val.as_u64() {
                                line = Some(n as u32);
                            } else if let Some(s) = val.as_str() {
                                line = s.parse::<u32>().ok();
                            }
                        }
                        _ => {
                            let _: serde_json::Value = map.next_value()?;
                        }
                    }
                }

                let file = file.ok_or_else(|| de::Error::missing_field("file"))?;
                Ok(SkillspectorLocation { file, line })
            }
        }

        deserializer.deserialize_any(LocationVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillspectorIssue {
    pub id: String,
    pub severity: String,
    pub category: String,
    pub location: SkillspectorLocation,
    #[serde(default)]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskAssessment {
    pub score: u32,
    pub severity: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillspectorReport {
    pub risk_assessment: RiskAssessment,
    pub issues: Vec<SkillspectorIssue>,
}

pub fn find_python_3_12() -> Option<PathBuf> {
    if std::env::var("MAKAKOO_TEST_FORCE_PYTHON_MISSING").is_ok() {
        return None;
    }
    let paths = [
        PathBuf::from("/opt/homebrew/opt/python@3.12/bin/python3.12"),
        PathBuf::from("/usr/local/opt/python@3.12/bin/python3.12"),
    ];
    for p in &paths {
        if p.exists() {
            return Some(p.clone());
        }
    }
    if let Ok(out) = Command::new("which").arg("python3.12").output() {
        if out.status.success() {
            let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }
    None
}

pub fn find_uv() -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/bin/uv");
        if p.exists() {
            return Some(p);
        }
    }
    let paths = [
        PathBuf::from("/opt/homebrew/bin/uv"),
        PathBuf::from("/usr/local/bin/uv"),
    ];
    for p in &paths {
        if p.exists() {
            return Some(p.clone());
        }
    }
    if let Ok(out) = Command::new("which").arg("uv").output() {
        if out.status.success() {
            let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }
    None
}

pub fn check_dependencies() -> anyhow::Result<(PathBuf, PathBuf)> {
    let python = find_python_3_12().ok_or_else(|| {
        anyhow!("Python 3.12 is missing. Please install it: brew install python@3.12 uv")
    })?;
    let uv = find_uv()
        .ok_or_else(|| anyhow!("uv is missing. Please install it: brew install python@3.12 uv"))?;
    Ok((python, uv))
}

pub fn bootstrap(no_cache: bool) -> anyhow::Result<PathBuf> {
    let home = makakoo_home();
    let venv_dir = home.join("state").join("skillspector-venv");
    let installed_ref_path = venv_dir.join("installed_ref.txt");
    let config = SkillSecurityConfig::load();
    let pinned_ref = &config.skillspector.pinned_ref;

    let needs_bootstrap = no_cache
        || !venv_dir.is_dir()
        || !installed_ref_path.is_file()
        || fs::read_to_string(&installed_ref_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
            != *pinned_ref;

    if needs_bootstrap {
        let (python_path, uv_path) = check_dependencies()?;

        if venv_dir.exists() {
            fs::remove_dir_all(&venv_dir).ok();
        }
        fs::create_dir_all(&venv_dir)?;

        let status = Command::new(&uv_path)
            .arg("venv")
            .arg(&venv_dir)
            .arg("--python")
            .arg(&python_path)
            .status()
            .map_err(|e| anyhow!("failed to initialize venv: {e}"))?;
        if !status.success() {
            return Err(anyhow!("uv venv creation failed"));
        }

        let python_bin = if cfg!(windows) {
            venv_dir.join("Scripts").join("python.exe")
        } else {
            venv_dir.join("bin").join("python")
        };

        let repo_url = format!("git+{}@{}", config.skillspector.pinned_git, pinned_ref);
        let status = Command::new(&uv_path)
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&python_bin)
            .arg(&repo_url)
            .status()
            .map_err(|e| anyhow!("failed to run uv pip install: {e}"))?;
        if !status.success() {
            return Err(anyhow!("uv pip install of SkillSpector failed"));
        }

        fs::write(&installed_ref_path, pinned_ref)?;
    }

    let skillspector_bin = if cfg!(windows) {
        venv_dir.join("Scripts").join("skillspector.exe")
    } else {
        venv_dir.join("bin").join("skillspector")
    };

    if !skillspector_bin.exists() {
        return Err(anyhow!(
            "skillspector executable not found in venv at {:?}",
            skillspector_bin
        ));
    }

    Ok(skillspector_bin)
}

pub fn get_skillspector_bin(no_cache: bool) -> anyhow::Result<PathBuf> {
    if let Ok(test_bin) = std::env::var("MAKAKOO_TEST_SKILLSPECTOR_BIN") {
        return Ok(PathBuf::from(test_bin));
    }
    bootstrap(no_cache)
}

fn slugify(target: &str) -> String {
    let cleaned = target.trim_end_matches('/');
    let last_part = cleaned.split('/').next_back().unwrap_or("unknown");
    let mut slug = String::new();
    for c in last_part.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if c == '-' || c == '_' || c == '.' {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug.to_string()
    }
}

pub struct ScanOptions {
    pub target: String,
    pub no_llm: bool,
    /// Reuse an existing report for today's target slug when present.
    ///
    /// Manual audits can opt into this for speed. Install-time security gates
    /// must set this to false so a stale report for `plugins/.stage/<name>`
    /// cannot block or bless changed plugin bytes.
    pub use_report_cache: bool,
    /// Force fresh SkillSpector bootstrap. Also bypasses report reuse.
    pub no_cache: bool,
    pub sarif_path: Option<PathBuf>,
}

pub fn run_scan(options: &ScanOptions) -> anyhow::Result<(SkillspectorReport, PathBuf)> {
    let bin_path = get_skillspector_bin(options.no_cache)?;

    let home = makakoo_home();
    let date_str = Utc::now().format("%Y-%m-%d").to_string();
    let slug = slugify(&options.target);
    let config = SkillSecurityConfig::load();
    let report_dir_raw = &config.skillspector.report_dir;
    let report_dir_base =
        PathBuf::from(report_dir_raw.replace("$MAKAKOO_HOME", &home.to_string_lossy()));
    let daily_dir = report_dir_base.join(&date_str);
    fs::create_dir_all(&daily_dir)?;

    let report_json_path = daily_dir.join(format!("{}.json", slug));
    let report_sarif_path = daily_dir.join(format!("{}.sarif", slug));

    if options.use_report_cache && !options.no_cache && report_json_path.exists() {
        if let Ok(json_content) = fs::read_to_string(&report_json_path) {
            if let Ok(report) = serde_json::from_str::<SkillspectorReport>(&json_content) {
                if let Some(user_sarif_path) = &options.sarif_path {
                    if report_sarif_path.exists() {
                        fs::copy(&report_sarif_path, user_sarif_path).ok();
                    }
                }
                return Ok((report, report_json_path));
            }
        }
    }

    // Avoid accidentally accepting a stale report if the scanner exits before
    // writing the new output. SkillSpector returns a non-zero status for some
    // real findings, so the existence + parseability of the fresh report is
    // the signal we care about, not exit status alone.
    let _ = fs::remove_file(&report_json_path);
    let _ = fs::remove_file(&report_sarif_path);

    // 1. Run for JSON
    let mut cmd = skillspector_command(&bin_path);
    cmd.arg("scan");
    cmd.arg(&options.target);
    cmd.arg("--format").arg("json");
    cmd.arg("--output").arg(&report_json_path);
    if options.no_llm {
        cmd.arg("--no-llm");
    }

    let status = cmd
        .status()
        .map_err(|e| anyhow!("failed to run skillspector scan for JSON: {e}"))?;

    let json_content = fs::read_to_string(&report_json_path).with_context(|| {
        format!(
            "skillspector scan JSON command failed ({}) and did not write {}",
            status,
            report_json_path.display()
        )
    })?;
    let report: SkillspectorReport =
        serde_json::from_str(&json_content).context("Failed to parse SkillSpector JSON report")?;

    // 2. Run for SARIF
    let mut cmd_sarif = skillspector_command(&bin_path);
    cmd_sarif.arg("scan");
    cmd_sarif.arg(&options.target);
    cmd_sarif.arg("--format").arg("sarif");
    cmd_sarif.arg("--output").arg(&report_sarif_path);
    if options.no_llm {
        cmd_sarif.arg("--no-llm");
    }

    let status_sarif = cmd_sarif
        .status()
        .map_err(|e| anyhow!("failed to run skillspector scan for SARIF: {e}"))?;
    if !status_sarif.success() && !report_sarif_path.exists() {
        return Err(anyhow!(
            "skillspector scan SARIF command failed ({}) and did not write {}",
            status_sarif,
            report_sarif_path.display()
        ));
    }

    // 3. Save audit log
    save_audit_entry(&options.target, &report, &report_json_path, options.no_llm)?;

    // 4. Copy SARIF to requested path if specified
    if let Some(user_sarif_path) = &options.sarif_path {
        fs::copy(&report_sarif_path, user_sarif_path)?;
    }

    Ok((report, report_json_path))
}

fn skillspector_command(bin_path: &Path) -> Command {
    #[cfg(all(test, windows))]
    {
        if bin_path.extension().and_then(|ext| ext.to_str()) == Some("py") {
            let mut cmd = Command::new("python");
            cmd.arg(bin_path);
            return cmd;
        }
    }

    Command::new(bin_path)
}

fn save_audit_entry(
    target: &str,
    report: &SkillspectorReport,
    report_json_path: &Path,
    no_llm: bool,
) -> anyhow::Result<()> {
    let home = makakoo_home();
    let audit_log_path = home.join("logs").join("audit.jsonl");
    if let Some(parent) = audit_log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut critical_count = 0;
    let mut high_count = 0;
    let mut medium_count = 0;
    let mut low_count = 0;

    for issue in &report.issues {
        match issue.severity.to_ascii_uppercase().as_str() {
            "CRITICAL" => critical_count += 1,
            "HIGH" => high_count += 1,
            "MEDIUM" => medium_count += 1,
            "LOW" => low_count += 1,
            _ => {}
        }
    }

    let source_kind = if target.starts_with("http://") || target.starts_with("https://") {
        if target.contains(".git") || target.contains("github.com") {
            "git".to_string()
        } else {
            "url".to_string()
        }
    } else if target.ends_with(".zip") {
        "zip".to_string()
    } else {
        let p = Path::new(target);
        if p.is_file() {
            "file".to_string()
        } else {
            "path".to_string()
        }
    };

    let audit_event = serde_json::json!({
        "ts": Utc::now().to_rfc3339(),
        "event": "skillspector.scan",
        "target": target,
        "source_kind": source_kind,
        "score": report.risk_assessment.score,
        "severity": report.risk_assessment.severity,
        "recommendation": report.risk_assessment.recommendation,
        "findings": {
            "critical": critical_count,
            "high": high_count,
            "medium": medium_count,
            "low": low_count
        },
        "report_path": report_json_path.to_string_lossy(),
        "tool_version": "2.1.1",
        "mode": if no_llm { "static" } else { "llm" }
    });

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_log_path)?;

    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(&audit_event)?)?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FleetSeverityCounts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetTargetReport {
    pub target: String,
    pub score: u32,
    pub severity: String,
    pub issues_count: usize,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetSummary {
    pub timestamp: String,
    pub total_scanned: usize,
    pub severity_counts: FleetSeverityCounts,
    pub targets: Vec<FleetTargetReport>,
}

impl FleetSummary {
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Skill Security Fleet Audit Summary\n\n");
        md.push_str(&format!("*Audit performed at: {}*\n\n", self.timestamp));
        md.push_str("## Overall Metrics\n\n");
        md.push_str(&format!(
            "- **Total Targets Scanned:** {}\n",
            self.total_scanned
        ));
        md.push_str("- **Severity Counts:**\n");
        md.push_str(&format!(
            "  - **Critical:** {}\n",
            self.severity_counts.critical
        ));
        md.push_str(&format!("  - **High:** {}\n", self.severity_counts.high));
        md.push_str(&format!(
            "  - **Medium:** {}\n",
            self.severity_counts.medium
        ));
        md.push_str(&format!("  - **Low:** {}\n\n", self.severity_counts.low));

        let mut sorted_targets = self.targets.clone();
        sorted_targets.sort_by_key(|t| std::cmp::Reverse(t.score));

        if let Some(worst) = sorted_targets.first() {
            if worst.score > 0 {
                md.push_str("## Worst Risk Target\n\n");
                md.push_str(&format!("- **Target:** `{}`\n", worst.target));
                md.push_str(&format!("- **Severity:** {}\n", worst.severity));
                md.push_str(&format!("- **Score:** {}/100\n\n", worst.score));
            }
        }

        md.push_str("## Scanned Targets\n\n");
        md.push_str("| Target | Score | Severity | Issues | Status |\n");
        md.push_str("| --- | --- | --- | --- | --- |\n");
        for t in &self.targets {
            let status = if let Some(err) = &t.error {
                format!("ERROR: {err}")
            } else if t.score >= 70 {
                "BLOCKED".to_string()
            } else {
                "ALLOWED".to_string()
            };
            md.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                t.target, t.score, t.severity, t.issues_count, status
            ));
        }

        md
    }
}

pub fn discover_skill_roots(start_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut dirs_to_visit = vec![start_dir.to_path_buf()];

    while let Some(dir) = dirs_to_visit.pop() {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            if name == ".git"
                || name == ".venv"
                || name == "node_modules"
                || name == "target"
                || name == "dist"
            {
                continue;
            }
        }

        let has_manifest = dir.join("plugin.toml").exists() || dir.join("SKILL.md").exists();
        if has_manifest {
            roots.push(dir.clone());
        }

        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs_to_visit.push(path);
                }
            }
        }
    }

    roots
}

pub fn discover_fleet_targets(start_dir: &Path, home: &Path) -> Vec<PathBuf> {
    let mut targets = std::collections::BTreeSet::new();

    let mut roots = vec![home.join("plugins"), home.join("skills-shared")];

    if std::env::var_os("MAKAKOO_TEST_NO_USER_SKILL_ROOTS").is_none() {
        if let Some(user_home) = dirs::home_dir() {
            roots.push(user_home.join(".agents").join("skills"));
            roots.push(user_home.join(".codex").join("skills"));
            roots.push(user_home.join(".claude").join("skills"));
            roots.push(user_home.join(".lope").join("skills"));
        }
    }

    // Include the caller CWD as a local ad-hoc root so tests and project-local
    // skills are auditable, but dedupe against canonical configured roots.
    roots.push(start_dir.to_path_buf());

    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for skill_root in discover_skill_roots(&root) {
            if let Ok(canonical) = skill_root.canonicalize() {
                targets.insert(canonical);
            }
        }
    }

    targets.into_iter().collect()
}

pub fn run_fleet_scan(
    targets: &[PathBuf],
    options: &ScanOptions,
) -> anyhow::Result<(FleetSummary, PathBuf, PathBuf)> {
    let mut fleet_targets = Vec::new();
    let mut severity_counts = FleetSeverityCounts {
        critical: 0,
        high: 0,
        medium: 0,
        low: 0,
    };

    for target_path in targets {
        let target_str = target_path.to_string_lossy().to_string();
        let target_opts = ScanOptions {
            target: target_str.clone(),
            no_llm: options.no_llm,
            use_report_cache: options.use_report_cache,
            no_cache: options.no_cache,
            sarif_path: None,
        };

        match run_scan(&target_opts) {
            Ok((report, _)) => {
                for issue in &report.issues {
                    match issue.severity.to_ascii_uppercase().as_str() {
                        "CRITICAL" => {
                            severity_counts.critical += 1;
                        }
                        "HIGH" => {
                            severity_counts.high += 1;
                        }
                        "MEDIUM" => {
                            severity_counts.medium += 1;
                        }
                        "LOW" => {
                            severity_counts.low += 1;
                        }
                        _ => {}
                    }
                }

                fleet_targets.push(FleetTargetReport {
                    target: target_str,
                    score: report.risk_assessment.score,
                    severity: report.risk_assessment.severity,
                    issues_count: report.issues.len(),
                    error: None,
                });
            }
            Err(e) => {
                fleet_targets.push(FleetTargetReport {
                    target: target_str,
                    score: 0,
                    severity: "UNKNOWN".to_string(),
                    issues_count: 0,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    let summary = FleetSummary {
        timestamp: Utc::now().to_rfc3339(),
        total_scanned: fleet_targets.len(),
        severity_counts,
        targets: fleet_targets,
    };

    let home = makakoo_home();
    let config = SkillSecurityConfig::load();
    let report_dir_raw = &config.skillspector.report_dir;
    let report_dir_base =
        PathBuf::from(report_dir_raw.replace("$MAKAKOO_HOME", &home.to_string_lossy()));
    let date_str = Utc::now().format("%Y-%m-%d").to_string();
    let report_dir = report_dir_base.join(&date_str);
    fs::create_dir_all(&report_dir)?;

    let json_path = report_dir.join("fleet-summary.json");
    let md_path = report_dir.join("fleet-summary.md");

    let json_content = serde_json::to_string_pretty(&summary)?;
    fs::write(&json_path, json_content)?;

    let md_content = summary.to_markdown();
    fs::write(&md_path, md_content)?;

    Ok((summary, json_path, md_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("gstack/browse"), "browse");
        assert_eq!(slugify("/Users/test/my-skill-plugin/"), "my-skill-plugin");
        assert_eq!(
            slugify("https://github.com/nvidia/skillspector"),
            "skillspector"
        );
        assert_eq!(slugify(""), "unknown");
    }

    #[test]
    fn test_parse_low_report() {
        let data = r#"{
            "risk_assessment": {
                "score": 0,
                "severity": "LOW",
                "recommendation": "SAFE"
            },
            "issues": []
        }"#;
        let report: SkillspectorReport = serde_json::from_str(data).unwrap();
        assert_eq!(report.risk_assessment.score, 0);
        assert_eq!(report.risk_assessment.severity, "LOW");
        assert_eq!(report.risk_assessment.recommendation, "SAFE");
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_parse_critical_report_with_location_varieties() {
        let data = r#"{
            "risk_assessment": {
                "score": 98,
                "severity": "CRITICAL",
                "recommendation": "REVIEW REQUIRED"
            },
            "issues": [
                {
                    "id": "PE3",
                    "severity": "HIGH",
                    "category": "Credential Access",
                    "location": {
                        "file": "test/cookie-import-browser.test.ts",
                        "line": 25
                    },
                    "details": "keychain"
                },
                {
                    "id": "TM1",
                    "severity": "HIGH",
                    "category": "Tool Parameter Abuse",
                    "location": "SKILL.md:23"
                }
            ]
        }"#;
        let report: SkillspectorReport = serde_json::from_str(data).unwrap();
        assert_eq!(report.risk_assessment.score, 98);
        assert_eq!(report.risk_assessment.severity, "CRITICAL");
        assert_eq!(report.issues.len(), 2);

        assert_eq!(report.issues[0].id, "PE3");
        assert_eq!(
            report.issues[0].location.file,
            "test/cookie-import-browser.test.ts"
        );
        assert_eq!(report.issues[0].location.line, Some(25));
        assert_eq!(report.issues[0].details.as_deref(), Some("keychain"));

        assert_eq!(report.issues[1].id, "TM1");
        assert_eq!(report.issues[1].location.file, "SKILL.md");
        assert_eq!(report.issues[1].location.line, Some(23));
        assert_eq!(report.issues[1].details, None);
    }

    #[test]
    fn test_python_discovery_override() {
        std::env::set_var("MAKAKOO_TEST_FORCE_PYTHON_MISSING", "1");
        assert!(find_python_3_12().is_none());
        std::env::remove_var("MAKAKOO_TEST_FORCE_PYTHON_MISSING");
    }

    #[test]
    fn test_discover_fleet_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path();

        let p1 = path.join("plugins").join("p1");
        fs::create_dir_all(&p1).unwrap();
        fs::write(p1.join("plugin.toml"), "").unwrap();

        let local_skill = path.join("local-skill");
        fs::create_dir_all(&local_skill).unwrap();
        fs::write(local_skill.join("SKILL.md"), "").unwrap();

        let vendor = path.join("local-skill").join("node_modules");
        fs::create_dir_all(&vendor).unwrap();
        fs::write(vendor.join("SKILL.md"), "").unwrap(); // should be ignored

        std::env::set_var("MAKAKOO_TEST_NO_USER_SKILL_ROOTS", "1");
        let targets = discover_fleet_targets(path, path);
        std::env::remove_var("MAKAKOO_TEST_NO_USER_SKILL_ROOTS");
        assert_eq!(targets.len(), 2);

        let canon_p1 = p1.canonicalize().unwrap();
        let canon_local = local_skill.canonicalize().unwrap();
        assert!(targets.contains(&canon_p1));
        assert!(targets.contains(&canon_local));
    }
}
