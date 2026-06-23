//! `makakoo skill audit` — manual security audit command handler.

use crate::context::CliContext;
use clap::error::ErrorKind;
use clap::Parser;
use makakoo_core::skill_security::{
    discover_fleet_targets, run_fleet_scan, run_scan, FleetSummary, ScanOptions, SkillspectorReport,
};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "makakoo skill audit", no_binary_name = true)]
pub struct AuditCli {
    /// Target path or URL to scan
    pub target: Option<String>,

    /// Print report in JSON format to stdout
    #[arg(long)]
    pub json: bool,

    /// Output SARIF report to the specified path
    #[arg(long)]
    pub sarif: Option<String>,

    /// Force fresh bootstrap of the SkillSpector virtual environment
    #[arg(long)]
    pub no_cache: bool,

    /// Skip LLM analysis (static mode is the default; kept for explicitness)
    #[arg(long)]
    pub no_llm: bool,

    /// Enable optional SkillSpector LLM semantic analysis
    #[arg(long, conflicts_with = "no_llm")]
    pub llm: bool,

    /// Scan all installed plugins and local skill roots
    #[arg(long)]
    pub all: bool,

    /// Limit the number of targets scanned
    #[arg(long)]
    pub limit: Option<usize>,
}

pub async fn run_audit(args: &[String], ctx: &CliContext) -> anyhow::Result<i32> {
    let cli = match AuditCli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return Ok(match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 1,
            });
        }
    };

    if cli.all {
        return run_fleet_audit(&cli, ctx).await;
    }

    let target = match &cli.target {
        Some(t) => t.clone(),
        None => {
            eprintln!("error: target is required unless --all is specified");
            return Ok(1);
        }
    };

    let options = ScanOptions {
        target: target.clone(),
        no_llm: !cli.llm || cli.no_llm,
        use_report_cache: true,
        no_cache: cli.no_cache,
        sarif_path: cli.sarif.clone().map(PathBuf::from),
    };

    let (report, report_path) = match run_scan(&options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error running security audit: {}", e);
            return Ok(1);
        }
    };

    if cli.json {
        let json_pretty = serde_json::to_string_pretty(&report)?;
        println!("{}", json_pretty);
    } else {
        print_terminal_summary(&target, &report, &report_path)?;
    }

    Ok(0)
}

pub async fn run_fleet_audit(cli: &AuditCli, _ctx: &CliContext) -> anyhow::Result<i32> {
    let home = makakoo_core::platform::makakoo_home();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let mut targets = discover_fleet_targets(&cwd, &home);

    if let Some(limit) = cli.limit {
        if targets.len() > limit {
            targets.truncate(limit);
        }
    }

    let options = ScanOptions {
        target: "".to_string(),
        no_llm: !cli.llm || cli.no_llm,
        use_report_cache: true,
        no_cache: cli.no_cache,
        sarif_path: None,
    };

    let (summary, json_path, md_path) = match run_fleet_scan(&targets, &options) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Error running fleet audit: {}", e);
            return Ok(1);
        }
    };

    if cli.json {
        let json_pretty = serde_json::to_string_pretty(&summary)?;
        println!("{}", json_pretty);
    } else {
        print_fleet_terminal_summary(&summary, &json_path, &md_path)?;
    }

    Ok(0)
}

fn print_fleet_terminal_summary(
    summary: &FleetSummary,
    json_path: &std::path::Path,
    md_path: &std::path::Path,
) -> anyhow::Result<()> {
    let home = makakoo_core::platform::makakoo_home();
    let home_display = home.to_string_lossy();

    let json_display = json_path
        .to_string_lossy()
        .replace(home_display.as_ref(), "$MAKAKOO_HOME");
    let md_display = md_path
        .to_string_lossy()
        .replace(home_display.as_ref(), "$MAKAKOO_HOME");

    println!("Skill Security Fleet Audit");
    println!("==========================");
    println!("Total targets scanned: {}", summary.total_scanned);
    println!(
        "Findings: CRITICAL {}, HIGH {}, MEDIUM {}, LOW {}",
        summary.severity_counts.critical,
        summary.severity_counts.high,
        summary.severity_counts.medium,
        summary.severity_counts.low
    );
    println!();

    let mut sorted_targets = summary.targets.clone();
    sorted_targets.sort_by_key(|b| std::cmp::Reverse(b.score));

    if let Some(worst) = sorted_targets.first() {
        if worst.score > 0 {
            println!("Worst risk target:");
            println!("  Target:   {}", worst.target);
            println!("  Severity: {}", worst.severity.to_ascii_uppercase());
            println!("  Score:    {}/100", worst.score);
            println!();
        }
    }

    let top_targets: Vec<_> = sorted_targets
        .iter()
        .filter(|t| t.score > 0)
        .take(5)
        .collect();

    if !top_targets.is_empty() {
        println!("Top risk targets:");
        for (i, t) in top_targets.iter().enumerate() {
            println!(
                "  {}. {} (Score: {}/100, Issues: {})",
                i + 1,
                t.target,
                t.score,
                t.issues_count
            );
        }
        println!();
    } else {
        println!("No risks found across scanned targets.\n");
    }

    println!("Fleet reports written to:");
    println!("  JSON:     {}", json_display);
    println!("  Markdown: {}", md_display);

    Ok(())
}

fn print_terminal_summary(
    target: &str,
    report: &SkillspectorReport,
    report_path: &std::path::Path,
) -> anyhow::Result<()> {
    let home = makakoo_core::platform::makakoo_home();
    let home_display = home.to_string_lossy();
    let report_display = report_path.to_string_lossy();
    let display_path = report_display.replace(home_display.as_ref(), "$MAKAKOO_HOME");

    println!("SkillSpector: {}", target);
    println!(
        "Risk: {} {}/100 — {}",
        report.risk_assessment.severity.to_ascii_uppercase(),
        report.risk_assessment.score,
        report.risk_assessment.recommendation
    );

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

    println!(
        "Findings: {} (CRITICAL {}, HIGH {}, MEDIUM {}, LOW {})",
        report.issues.len(),
        critical_count,
        high_count,
        medium_count,
        low_count
    );

    if !report.issues.is_empty() {
        println!("Top findings:");
        let mut sorted_issues = report.issues.clone();
        sorted_issues.sort_by(|a, b| {
            let severity_weight = |s: &str| match s.to_ascii_uppercase().as_str() {
                "CRITICAL" => 4,
                "HIGH" => 3,
                "MEDIUM" => 2,
                "LOW" => 1,
                _ => 0,
            };
            severity_weight(&b.severity).cmp(&severity_weight(&a.severity))
        });

        for issue in sorted_issues.iter().take(10) {
            let loc_str = if let Some(line) = issue.location.line {
                format!("{}:{}", issue.location.file, line)
            } else {
                issue.location.file.clone()
            };
            let details_suffix = if let Some(d) = &issue.details {
                format!(" ({})", d)
            } else {
                "".to_string()
            };
            println!(
                "  {} {} {}: {}{}",
                issue.severity.to_ascii_uppercase(),
                issue.id,
                loc_str,
                issue.category,
                details_suffix
            );
        }
    }
    println!();
    println!("Report: {}", display_path);
    println!(
        "Recommendation: {}",
        get_recommendation_msg(&report.risk_assessment.severity)
    );

    Ok(())
}

fn get_recommendation_msg(severity: &str) -> &'static str {
    match severity.to_ascii_uppercase().as_str() {
        "LOW" | "SAFE" => "Install allowed.",
        "MEDIUM" | "CAUTION" => "Warn; install allowed unless block_on=medium.",
        _ => "Do not install unknown sources unless reviewed. Known-local source may be overridden with --allow-risk."
    }
}
