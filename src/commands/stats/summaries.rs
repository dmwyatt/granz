//! `grans stats notes`: action items, summary templates, typed notes.

use colored::Colorize;

use crate::stats::notes::NotesReport;

/// How many series to list.
const SERIES_LISTED: usize = 10;

pub(super) fn print(report: &NotesReport, limit: usize) {
    if report.months.is_empty() {
        println!("No meetings found.");
        return;
    }
    println!(
        "{}",
        format!(
            "{:<7}  {:>8}  {:>11}  {:>12}",
            "month", "meetings", "typed notes", "action items"
        )
        .dimmed()
    );
    for m in &report.months {
        println!(
            "{}  {:>8}  {:>11}  {:>12}",
            m.month.format("%Y-%m"),
            m.meetings,
            m.with_typed_notes,
            m.action_items
        );
    }

    if !report.templates.is_empty() {
        println!();
        println!("{}", "Summary templates, in the order they appeared".bold());
        for t in &report.templates {
            println!(
                "{:>6}  {} to {}  {}",
                t.summaries,
                t.first.format("%Y-%m-%d"),
                t.last.format("%Y-%m-%d"),
                t.template
            );
        }
    }

    if !report.series.is_empty() {
        println!();
        println!("{}", "Action items per series, most first".bold());
        println!(
            "{}",
            format!("{:>5}  {:>8}  {:>5}  series", "items", "meetings", "yours").dimmed()
        );
        for s in report.series.iter().take(SERIES_LISTED) {
            println!(
                "{:>5}  {:>8}  {:>5}  {}",
                s.action_items, s.meetings, s.naming_me, s.title
            );
        }
    }

    println!();
    println!(
        "{} {}",
        "Action items naming you:".bold(),
        report.mine.len()
    );
    let shown = if limit == 0 { report.mine.len() } else { limit };
    for m in report.mine.iter().take(shown) {
        println!("  {}  {}", m.day.format("%Y-%m-%d"), m.item.text);
        println!("{}", format!("              {}", m.title).dimmed());
    }
    let hidden = report.mine.len().saturating_sub(shown);
    if hidden > 0 {
        println!(
            "{}",
            format!("  ... and {hidden} earlier (--limit 0 shows all)").dimmed()
        );
    }
}
