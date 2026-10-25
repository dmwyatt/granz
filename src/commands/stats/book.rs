//! `grans stats records`: the record book.

use chrono::FixedOffset;
use colored::Colorize;

use super::day;
use crate::stats::records::Record;

pub(super) fn print(records: &[Record], tz: &FixedOffset) {
    if records.is_empty() {
        println!("No meetings found.");
        return;
    }
    let width = records.iter().map(|r| r.name.len()).max().unwrap_or(0);
    for r in records {
        let where_ = match (&r.meeting, r.day) {
            (Some(m), _) => format!("{}  {}", day(m.created_at, tz), m.title),
            (None, Some(d)) => d.format("%Y-%m-%d").to_string(),
            (None, None) => String::new(),
        };
        let who = r
            .who
            .as_deref()
            .map_or(String::new(), |w| format!(" by {w}"));
        println!(
            "{:<width$}  {}{}  {}",
            r.name.bold(),
            value(r),
            who,
            where_.dimmed()
        );
    }
}

fn value(r: &Record) -> String {
    match r.unit {
        "o'clock" => {
            let minutes = (r.value * 60.0).round() as i64;
            format!("{:02}:{:02}", minutes / 60, minutes % 60)
        }
        "minutes" | "characters" | "handoffs per hour" => format!("{:.0} {}", r.value, r.unit),
        "hours" => format!("{:.1} {}", r.value, r.unit),
        _ => format!("{} {}", r.value, r.unit),
    }
}
