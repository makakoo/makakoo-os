//! `makakoo memory` subcommands — legacy-path purge and diagnostics.

use anyhow::Context;

use makakoo_core::superbrain::diagnostics::{compute_stats, MemoryStats};
use makakoo_core::superbrain::recall::{migrate_legacy_paths, LegacyPathReport};

use crate::cli::MemoryCmd;
use crate::context::CliContext;

pub async fn run(ctx: &CliContext, cmd: MemoryCmd) -> anyhow::Result<i32> {
    match cmd {
        MemoryCmd::PurgeLegacy { dry_run } => purge_legacy(ctx, dry_run),
        MemoryCmd::Stats { json } => stats(ctx, json),
    }
}

fn purge_legacy(ctx: &CliContext, dry_run: bool) -> anyhow::Result<i32> {
    let store = ctx.store().context("opening superbrain store")?;
    let conn_arc = store.conn_arc();
    let conn = conn_arc
        .lock()
        .map_err(|_| anyhow::anyhow!("superbrain connection mutex poisoned"))?;
    let report = migrate_legacy_paths(&conn, dry_run).context("migrating legacy paths")?;
    print_report(&report, dry_run);
    Ok(0)
}

fn print_report(r: &LegacyPathReport, dry_run: bool) {
    let mode = if dry_run { "DRY RUN — " } else { "" };
    println!("{mode}Legacy HARVEY-path migration report");
    println!("  recall_log rows rewritten:         {}", r.recall_log_rewritten);
    println!("  recall_stats rows rewritten:       {}", r.recall_stats_rewritten);
    println!("  memory_promotions rows rewritten:  {}", r.memory_promotions_rewritten);
    println!("  recall_stats rows deduped:         {}", r.recall_stats_deduped);
    if dry_run {
        println!();
        println!("Re-run without --dry-run to apply.");
    }
}

fn stats(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    let store = ctx.store().context("opening superbrain store")?;
    let conn_arc = store.conn_arc();
    let conn = conn_arc
        .lock()
        .map_err(|_| anyhow::anyhow!("superbrain connection mutex poisoned"))?;
    let stats = compute_stats(&conn).context("computing memory stats")?;
    drop(conn);

    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        print_stats_human(&stats);
    }
    Ok(0)
}

fn print_stats_human(s: &MemoryStats) {
    let t = &s.thresholds;
    println!("Recall log:");
    println!("  total entries:        {}", s.recall_log.total);
    println!("  today:                {}", s.recall_log.today);
    println!("  last 7d:              {}", s.recall_log.last_7d);
    if s.recall_log.by_source.is_empty() {
        println!("  by source:            (none)");
    } else {
        println!("  by source:");
        for (src, n) in &s.recall_log.by_source {
            println!("    {src:<32} {n}");
        }
    }
    println!();
    println!("Recall stats (promoter input):");
    println!(
        "  total content_hashes:                         {}",
        s.recall_stats.total_content_hashes
    );
    println!(
        "  passing MIN_RECALL_COUNT({}):                   {}",
        t.min_recall_count, s.recall_stats.passing_min_recall_count
    );
    println!(
        "  passing MIN_UNIQUE_QUERIES({}):                 {}",
        t.min_unique_queries, s.recall_stats.passing_min_unique_queries
    );
    println!(
        "  passing MAX_AGE_DAYS({}):                      {}",
        t.max_age_days, s.recall_stats.passing_max_age_days
    );
    println!(
        "  (MIN_SCORE({:.2}) applied inside promoter — see tracing)",
        t.min_score
    );
    println!();
    println!("Memory promotions:");
    println!("  total:        {}", s.memory_promotions.total);
    println!("  last 7d:      {}", s.memory_promotions.last_7d);
    match &s.memory_promotions.last_promoted_at {
        Some(ts) => println!("  last at:      {ts}"),
        None => println!("  last at:      (never)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed_harvey(conn: &Connection) {
        conn.execute(
            "INSERT INTO recall_log
                (doc_id, doc_path, content_hash, query_hash, score, source)
             VALUES (1, '/Users/sebastian/HARVEY/data/Brain/x.md',
                     'h1', 'q1', 0.9, 'cli')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recall_stats
                (content_hash, doc_id, doc_path, recall_count, unique_queries,
                 unique_days, total_score, max_score)
             VALUES ('h1', 1, '/Users/sebastian/HARVEY/data/Brain/x.md',
                     3, 2, 2, 2.5, 0.9)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn purge_legacy_dry_run_reports_counts_without_rewriting() {
        use makakoo_core::db::{open_db, run_migrations};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sb.db");
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        seed_harvey(&conn);
        let report = migrate_legacy_paths(&conn, true).unwrap();
        assert_eq!(report.recall_log_rewritten, 1);
        assert_eq!(report.recall_stats_rewritten, 1);
        // Row unchanged.
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall_log WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn purge_legacy_live_run_rewrites_rows() {
        use makakoo_core::db::{open_db, run_migrations};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sb.db");
        let conn = open_db(&path).unwrap();
        run_migrations(&conn).unwrap();
        seed_harvey(&conn);
        let report = migrate_legacy_paths(&conn, false).unwrap();
        assert_eq!(report.recall_log_rewritten, 1);
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall_log WHERE doc_path LIKE '/Users/sebastian/HARVEY/%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        let rewritten: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall_log WHERE doc_path LIKE '/Users/sebastian/MAKAKOO/%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rewritten, 1);
    }
}
