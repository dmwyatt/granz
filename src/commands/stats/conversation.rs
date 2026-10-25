//! `grans stats talk`: how much you talk, and how conversations flow.

use chrono::FixedOffset;
use colored::Colorize;

use super::{day, percent};
use crate::stats::talk::{MeetingRef, ShareGroup, TalkReport};

/// How many silent meetings to name.
const LISTED: usize = 10;

pub(super) fn print(report: &TalkReport, tz: &FixedOffset) {
    if report.overall.meetings == 0 {
        println!("No meetings with a transcript found.");
        return;
    }
    println!("{}", heading("").dimmed());
    print_group(&report.overall);
    println!();
    println!("{}", "By invitees who did not decline".bold());
    println!("{}", heading("invitees").dimmed());
    for g in &report.by_size {
        print_group(g);
    }
    println!();
    println!("{}", "By month".bold());
    println!("{}", heading("month").dimmed());
    for g in &report.by_month {
        print_group(g);
    }
    println!(
        "{}",
        "you: your share of what was said. back and forth: median handoffs between\n\
         you and everyone else per hour. questions: share of utterances ending in ?"
            .dimmed()
    );
    println!();
    print_tenths(&report.by_tenth);
    println!();
    print_list("Meetings where you never spoke", &report.silent, tz);
    print_list(
        "Meetings with only your microphone (often in person)",
        &report.microphone_only,
        tz,
    );
}

fn heading(first: &str) -> String {
    format!(
        "{:<10}  {:>8}  {:>4}  {:>14}  {:>13}  {:>16}",
        first, "meetings", "you", "back and forth", "you question", "others question"
    )
}

fn rate(r: Option<f64>) -> String {
    r.map_or("-".to_string(), |r| format!("{:.1}%", r * 100.0))
}

fn print_group(g: &ShareGroup) {
    println!(
        "{:<10}  {:>8}  {:>4}  {:>14}  {:>13}  {:>16}",
        g.label,
        g.meetings,
        g.my_share.map_or("-".to_string(), percent),
        g.handoffs_per_hour
            .map_or("-".to_string(), |h| format!("{h:.0}/h")),
        rate(g.my_question_rate),
        rate(g.others_question_rate)
    );
}

fn print_tenths(tenths: &[Option<f64>]) {
    println!(
        "{}",
        "Your share in each tenth of a meeting, first to last".bold()
    );
    let cells: Vec<String> = tenths
        .iter()
        .map(|t| t.map_or("-".to_string(), percent))
        .collect();
    println!("  {}", cells.join("  "));
}

fn print_list(title: &str, meetings: &[MeetingRef], tz: &FixedOffset) {
    println!("{} {}", format!("{title}:").bold(), meetings.len());
    for m in meetings.iter().rev().take(LISTED) {
        println!("  {}  {}", day(m.created_at, tz), m.title);
    }
    if meetings.len() > LISTED {
        println!(
            "{}",
            format!(
                "  ... and {} earlier (--json lists all)",
                meetings.len() - LISTED
            )
            .dimmed()
        );
    }
}
