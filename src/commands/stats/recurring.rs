//! `grans stats series`: recurring meetings, most hours first.

use chrono::FixedOffset;
use colored::Colorize;

use super::{day, percent};
use crate::stats::series::{SeriesReport, SeriesRollup};

pub(super) fn print(report: &SeriesReport, limit: usize, tz: &FixedOffset) {
    if report.series.is_empty() && report.one_off_meetings == 0 {
        println!("No meetings found.");
        return;
    }
    println!(
        "Recurring: {} meetings in {} series, {:.1} hours. One-off: {} meetings, {:.1} hours.",
        report.recurring_meetings,
        report.series.len(),
        report.recurring_hours,
        report.one_off_meetings,
        report.one_off_hours
    );
    if report.series.is_empty() {
        return;
    }
    println!();
    println!(
        "{}",
        format!(
            "{:>8}  {:>6}  {:<10}  {:<10}  {:>6}  {:>9}  {:>5}  {:>7}  {:>4}  series",
            "meetings", "hours", "first", "last", "every", "invited", "start", "overrun", "you"
        )
        .dimmed()
    );
    let shown = if limit == 0 {
        report.series.len()
    } else {
        limit
    };
    for s in report.series.iter().take(shown) {
        print_row(s, tz);
    }
    let hidden = report.series.len().saturating_sub(shown);
    if hidden > 0 {
        println!(
            "{}",
            format!("... and {hidden} more (--limit 0 shows all)").dimmed()
        );
    }
    println!(
        "{}",
        "every: median days between meetings; invited: first meeting -> latest;\n\
         start, overrun: median minutes the talk began after the slot started and\n\
         ran past its end."
            .dimmed()
    );
}

fn print_row(s: &SeriesRollup, tz: &FixedOffset) {
    let every = s.every_days.map_or("-".to_string(), |d| format!("{d:.0}d"));
    let signed = |m: Option<f64>| m.map_or("-".to_string(), |m| format!("{m:+.0}"));
    println!(
        "{:>8}  {:>6.1}  {:<10}  {:<10}  {:>6}  {:>9}  {:>5}  {:>7}  {:>4}  {}",
        s.meetings,
        s.hours,
        day(s.first, tz),
        day(s.last, tz),
        every,
        format!("{} -> {}", s.invited_first, s.invited_latest),
        signed(s.typical_start_delay_minutes),
        signed(s.typical_overrun_minutes),
        s.my_share.map_or("-".to_string(), percent),
        s.title.bold()
    );
}
