//! PatrolRunner — scheduled quality-check jobs owned by mascots.
//!
//! A patrol is a small, read-only background check that runs every N hours
//! for a given mascot. When it finds something, it emits a `PatrolReport`
//! wrapped in the mascot's own voice so the flavor lines show up in the
//! event bus / CLI output.
//!
//! The trait is async so patrols can call the LLM client, hit the
//! superbrain store, or do file I/O without blocking the Tokio runtime.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::event_bus::PersistentEventBus;
use crate::superbrain::SuperbrainStore;

use super::registry::MascotRegistry;

/// A patrol job owned by one mascot.
#[async_trait]
pub trait PatrolJob: Send + Sync {
    /// The mascot that owns this patrol. Must match a name in the registry.
    fn mascot_name(&self) -> &str;

    /// Run the patrol once. Returns a `PatrolReport` (possibly with an
    /// empty `findings` list if nothing of note was detected).
    async fn run(&self, ctx: &PatrolContext) -> Result<PatrolReport>;
}

/// Context passed to every patrol. Holds Arc handles to the subsystems
/// patrols commonly need.
pub struct PatrolContext {
    pub store: Arc<SuperbrainStore>,
    pub bus: Arc<PersistentEventBus>,
    pub home: PathBuf,
}

/// A patrol's output — a list of findings plus a flavor line.
#[derive(Clone, Debug)]
pub struct PatrolReport {
    pub mascot: String,
    pub findings: Vec<String>,
    pub flavor_line: String,
    pub timestamp: DateTime<Utc>,
}

/// Runs all registered patrols and collects their reports.
pub struct PatrolRunner {
    #[allow(dead_code)]
    registry: Arc<MascotRegistry>,
    jobs: Vec<Box<dyn PatrolJob>>,
    ctx: Arc<PatrolContext>,
}

impl PatrolRunner {
    pub fn new(registry: Arc<MascotRegistry>, ctx: Arc<PatrolContext>) -> Self {
        Self {
            registry,
            jobs: Vec::new(),
            ctx,
        }
    }

    pub fn register_job(&mut self, job: Box<dyn PatrolJob>) {
        self.jobs.push(job);
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Run every registered patrol sequentially. Errors are swallowed into
    /// a failure report so one bad patrol doesn't break the others — the
    /// same discipline the event bus uses for subscriber panics.
    pub async fn run_all(&self) -> Vec<PatrolReport> {
        let mut out = Vec::with_capacity(self.jobs.len());
        for job in &self.jobs {
            match job.run(&self.ctx).await {
                Ok(report) => out.push(report),
                Err(err) => out.push(PatrolReport {
                    mascot: job.mascot_name().to_string(),
                    findings: vec![format!("patrol error: {err}")],
                    flavor_line: format!("{} stumbled mid-patrol.", job.mascot_name()),
                    timestamp: Utc::now(),
                }),
            }
        }
        out
    }
}

// ══════════════════════════════════════════════════════════════
//  Concrete example patrols
// ══════════════════════════════════════════════════════════════

/// OlibiaGuardianPatrol — spot-checks today's Brain journal for integrity.
///
/// Counts lines in `{home}/data/Brain/journals/YYYY_MM_DD.md` and flags
/// any line that does not start with the outliner marker (`- `) — matching
/// the Logseq convention from the Python source.
pub struct OlibiaGuardianPatrol;

#[async_trait]
impl PatrolJob for OlibiaGuardianPatrol {
    fn mascot_name(&self) -> &str {
        "Olibia"
    }

    async fn run(&self, ctx: &PatrolContext) -> Result<PatrolReport> {
        let today = Utc::now().format("%Y_%m_%d").to_string();
        let journal = ctx
            .home
            .join("data")
            .join("Brain")
            .join("journals")
            .join(format!("{today}.md"));

        let mut findings: Vec<String> = Vec::new();
        if !journal.exists() {
            findings.push(format!("no journal for {today}"));
        } else {
            let body = tokio::fs::read_to_string(&journal).await.unwrap_or_default();
            let total = body.lines().count();
            let malformed: Vec<usize> = body
                .lines()
                .enumerate()
                .filter_map(|(i, l)| {
                    let trimmed = l.trim_start();
                    if trimmed.is_empty() || trimmed.starts_with("- ") || trimmed.starts_with('#') {
                        None
                    } else {
                        Some(i + 1)
                    }
                })
                .collect();
            if !malformed.is_empty() {
                findings.push(format!(
                    "journal {total} lines, {} malformed (first at line {})",
                    malformed.len(),
                    malformed[0]
                ));
            }
        }

        let flavor = if findings.is_empty() {
            "Olibia perches quietly. Journal looks tidy.".to_string()
        } else {
            "Olibia tilts her head at the journal.".to_string()
        };

        Ok(PatrolReport {
            mascot: "Olibia".to_string(),
            findings,
            flavor_line: flavor,
            timestamp: Utc::now(),
        })
    }
}

/// PixelLogPatrol — greps recent log files under `{home}/data/logs/` for
/// the usual suspicious substrings.
pub struct PixelLogPatrol;

impl PixelLogPatrol {
    const PATTERNS: &'static [&'static str] = &["401", "403", "ERROR"];
}

#[async_trait]
impl PatrolJob for PixelLogPatrol {
    fn mascot_name(&self) -> &str {
        "Pixel"
    }

    async fn run(&self, ctx: &PatrolContext) -> Result<PatrolReport> {
        let logs_dir = ctx.home.join("data").join("logs");
        let mut findings: Vec<String> = Vec::new();

        if let Ok(mut entries) = tokio::fs::read_dir(&logs_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let body = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                for pat in Self::PATTERNS {
                    let hits = body.matches(pat).count();
                    if hits > 0 {
                        findings.push(format!(
                            "{}: {hits}x {pat}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
            }
        }

        let flavor = if findings.is_empty() {
            "Pixel ruffles feathers. Logs look clean.".to_string()
        } else {
            "Pixel chitters — something in the logs wants a second look.".to_string()
        };

        Ok(PatrolReport {
            mascot: "Pixel".to_string(),
            findings,
            flavor_line: flavor,
            timestamp: Utc::now(),
        })
    }
}

// ══════════════════════════════════════════════════════════════
//  Tests
// ══════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;

    fn make_ctx() -> (TempDir, Arc<PatrolContext>, Arc<MascotRegistry>) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let brain_db = dir.path().join("superbrain.db");
        let bus_db = dir.path().join("bus.db");
        let store = Arc::new(SuperbrainStore::open(&brain_db).unwrap());
        let bus = PersistentEventBus::open(&bus_db).unwrap();
        let registry =
            Arc::new(MascotRegistry::load(&dir.path().join("nursery.json")).unwrap());
        let ctx = Arc::new(PatrolContext {
            store,
            bus,
            home,
        });
        (dir, ctx, registry)
    }

    struct StubPatrol {
        name: &'static str,
    }

    #[async_trait]
    impl PatrolJob for StubPatrol {
        fn mascot_name(&self) -> &str {
            self.name
        }
        async fn run(&self, _ctx: &PatrolContext) -> Result<PatrolReport> {
            Ok(PatrolReport {
                mascot: self.name.to_string(),
                findings: vec!["stub finding".to_string()],
                flavor_line: format!("{} says hi.", self.name),
                timestamp: Utc::now(),
            })
        }
    }

    struct BustedPatrol;
    #[async_trait]
    impl PatrolJob for BustedPatrol {
        fn mascot_name(&self) -> &str {
            "Busted"
        }
        async fn run(&self, _ctx: &PatrolContext) -> Result<PatrolReport> {
            Err(crate::error::MakakooError::internal("boom"))
        }
    }

    #[test]
    fn run_all_returns_report_per_registered_job() {
        let rt = Runtime::new().unwrap();
        let (_d, ctx, registry) = make_ctx();
        let mut runner = PatrolRunner::new(registry, ctx);
        runner.register_job(Box::new(StubPatrol { name: "Olibia" }));
        runner.register_job(Box::new(StubPatrol { name: "Pixel" }));
        assert_eq!(runner.job_count(), 2);
        let reports = rt.block_on(runner.run_all());
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].mascot, "Olibia");
        assert_eq!(reports[1].mascot, "Pixel");
        for r in &reports {
            assert_eq!(r.findings.len(), 1);
            assert!(!r.flavor_line.is_empty());
        }
    }

    #[test]
    fn run_all_converts_errors_into_failure_reports() {
        let rt = Runtime::new().unwrap();
        let (_d, ctx, registry) = make_ctx();
        let mut runner = PatrolRunner::new(registry, ctx);
        runner.register_job(Box::new(StubPatrol { name: "Olibia" }));
        runner.register_job(Box::new(BustedPatrol));
        runner.register_job(Box::new(StubPatrol { name: "Pixel" }));
        let reports = rt.block_on(runner.run_all());
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[1].mascot, "Busted");
        assert!(reports[1].findings[0].contains("patrol error"));
        // Other patrols still ran to completion after the failure.
        assert_eq!(reports[0].mascot, "Olibia");
        assert_eq!(reports[2].mascot, "Pixel");
    }

    #[test]
    fn olibia_patrol_no_journal_is_a_finding() {
        let rt = Runtime::new().unwrap();
        let (_d, ctx, _reg) = make_ctx();
        let job = OlibiaGuardianPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert_eq!(report.mascot, "Olibia");
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].contains("no journal"));
    }

    #[test]
    fn olibia_patrol_clean_journal_has_zero_findings() {
        let rt = Runtime::new().unwrap();
        let (dir, ctx, _reg) = make_ctx();
        let today = Utc::now().format("%Y_%m_%d").to_string();
        let journal_dir = dir.path().join("data").join("Brain").join("journals");
        std::fs::create_dir_all(&journal_dir).unwrap();
        std::fs::write(
            journal_dir.join(format!("{today}.md")),
            "# Harvey Journal\n- first entry\n- second entry\n",
        )
        .unwrap();
        let job = OlibiaGuardianPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert!(
            report.findings.is_empty(),
            "expected no findings, got {:?}",
            report.findings
        );
        assert!(report.flavor_line.contains("tidy"));
    }

    #[test]
    fn olibia_patrol_catches_malformed_journal_lines() {
        let rt = Runtime::new().unwrap();
        let (dir, ctx, _reg) = make_ctx();
        let today = Utc::now().format("%Y_%m_%d").to_string();
        let journal_dir = dir.path().join("data").join("Brain").join("journals");
        std::fs::create_dir_all(&journal_dir).unwrap();
        std::fs::write(
            journal_dir.join(format!("{today}.md")),
            "- fine\nthis line is wrong\n- also fine\n",
        )
        .unwrap();
        let job = OlibiaGuardianPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].contains("malformed"));
    }

    #[test]
    fn pixel_patrol_finds_suspicious_patterns() {
        let rt = Runtime::new().unwrap();
        let (dir, ctx, _reg) = make_ctx();
        let logs_dir = dir.path().join("data").join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(
            logs_dir.join("api.log"),
            "request ok\nhttp 401 from upstream\nhttp 403 denied\nERROR out of memory\n",
        )
        .unwrap();
        let job = PixelLogPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert_eq!(report.mascot, "Pixel");
        // 3 patterns × 1 file with at least one hit each.
        assert_eq!(report.findings.len(), 3);
    }

    #[test]
    fn pixel_patrol_clean_logs_report_is_empty() {
        let rt = Runtime::new().unwrap();
        let (dir, ctx, _reg) = make_ctx();
        let logs_dir = dir.path().join("data").join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("api.log"), "request ok\nall green\n").unwrap();
        let job = PixelLogPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert!(report.findings.is_empty());
        assert!(report.flavor_line.contains("clean"));
    }

    #[test]
    fn pixel_patrol_missing_logs_dir_is_clean() {
        let rt = Runtime::new().unwrap();
        let (_d, ctx, _reg) = make_ctx();
        let job = PixelLogPatrol;
        let report = rt.block_on(job.run(&ctx)).unwrap();
        assert!(report.findings.is_empty());
    }
}
