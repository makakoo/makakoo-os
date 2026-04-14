//! Terminal formatting helpers — crossterm styling + comfy-table output.

use comfy_table::{presets::UTF8_FULL, Cell, Color as TableColor, Table};
use crossterm::style::Stylize;

use makakoo_core::nursery::Mascot;
use makakoo_core::sancho::HandlerReport;
use makakoo_core::superbrain::promoter::Promotion;
use makakoo_core::superbrain::store::SearchHit;

/// Render a set of FTS search hits as a table. The snippet column
/// trims content to 80 chars without breaking grapheme boundaries.
pub fn print_search_hits(hits: &[SearchHit]) {
    if hits.is_empty() {
        println!("{}", "(no hits)".dark_grey());
        return;
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("doc_id").fg(TableColor::Cyan),
        Cell::new("type").fg(TableColor::Cyan),
        Cell::new("score").fg(TableColor::Cyan),
        Cell::new("snippet").fg(TableColor::Cyan),
    ]);
    for h in hits {
        let snippet: String = h.content.chars().take(80).collect();
        t.add_row(vec![
            Cell::new(&h.doc_id).fg(TableColor::White),
            Cell::new(&h.doc_type).fg(TableColor::DarkYellow),
            Cell::new(format!("{:.3}", h.score)),
            Cell::new(snippet),
        ]);
    }
    println!("{t}");
}

/// Render the nursery as a table.
pub fn print_mascot_list(mascots: &[Mascot]) {
    if mascots.is_empty() {
        println!("{}", "(nursery empty)".dark_grey());
        return;
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("name").fg(TableColor::Cyan),
        Cell::new("species").fg(TableColor::Cyan),
        Cell::new("status").fg(TableColor::Cyan),
        Cell::new("maintainer").fg(TableColor::Cyan),
        Cell::new("job").fg(TableColor::Cyan),
    ]);
    for m in mascots {
        t.add_row(vec![
            Cell::new(&m.name).fg(TableColor::White),
            Cell::new(&m.species).fg(TableColor::DarkYellow),
            Cell::new(format!("{:?}", m.status)),
            Cell::new(&m.maintainer),
            Cell::new(&m.job),
        ]);
    }
    println!("{t}");
}

/// Render SANCHO handler reports.
pub fn print_handler_reports(reports: &[HandlerReport]) {
    if reports.is_empty() {
        println!("{}", "(no eligible handlers this tick)".dark_grey());
        return;
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("handler").fg(TableColor::Cyan),
        Cell::new("ok").fg(TableColor::Cyan),
        Cell::new("duration").fg(TableColor::Cyan),
        Cell::new("message").fg(TableColor::Cyan),
    ]);
    for r in reports {
        let ok_cell = if r.ok {
            Cell::new("ok").fg(TableColor::Green)
        } else {
            Cell::new("fail").fg(TableColor::Red)
        };
        t.add_row(vec![
            Cell::new(&r.handler).fg(TableColor::White),
            ok_cell,
            Cell::new(format!("{:.3}s", r.duration.as_secs_f64())),
            Cell::new(&r.message),
        ]);
    }
    println!("{t}");
}

/// Render memory-promotion candidates with component breakdown.
pub fn print_promotions(promos: &[Promotion]) {
    if promos.is_empty() {
        println!("{}", "(no promotion candidates)".dark_grey());
        return;
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(vec![
        Cell::new("doc_path").fg(TableColor::Cyan),
        Cell::new("score").fg(TableColor::Cyan),
        Cell::new("freq").fg(TableColor::Cyan),
        Cell::new("rel").fg(TableColor::Cyan),
        Cell::new("div").fg(TableColor::Cyan),
        Cell::new("rec").fg(TableColor::Cyan),
    ]);
    for p in promos {
        t.add_row(vec![
            Cell::new(&p.doc_path).fg(TableColor::White),
            Cell::new(format!("{:.3}", p.score)).fg(TableColor::Green),
            Cell::new(format!("{:.2}", p.components.frequency)),
            Cell::new(format!("{:.2}", p.components.relevance)),
            Cell::new(format!("{:.2}", p.components.diversity)),
            Cell::new(format!("{:.2}", p.components.recency)),
        ]);
    }
    println!("{t}");
}

/// Print a pre-formatted buddy frame (trust the renderer inside
/// BuddyTracker to produce something already styled).
pub fn print_buddy_frame(frame: &str) {
    print!("{frame}");
}

/// Print an `error:` prefix followed by a message on stderr.
pub fn print_error(msg: impl AsRef<str>) {
    eprintln!("{} {}", "error:".red().bold(), msg.as_ref());
}

/// Print a `warn:` prefix followed by a message on stderr.
pub fn print_warn(msg: impl AsRef<str>) {
    eprintln!("{} {}", "warn:".yellow().bold(), msg.as_ref());
}

/// Print an info line to stdout.
pub fn print_info(msg: impl AsRef<str>) {
    println!("{}", msg.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn empty_hits_does_not_panic() {
        print_search_hits(&[]);
    }

    #[test]
    fn hits_smoke() {
        let hit = SearchHit {
            doc_id: "pages/test.md".into(),
            content: "hello world".into(),
            doc_type: "page".into(),
            score: 0.99,
            metadata: Value::Null,
        };
        print_search_hits(&[hit]);
    }

    #[test]
    fn empty_mascots_does_not_panic() {
        print_mascot_list(&[]);
    }

    #[test]
    fn empty_reports_does_not_panic() {
        print_handler_reports(&[]);
    }

    #[test]
    fn empty_promotions_does_not_panic() {
        print_promotions(&[]);
    }
}
