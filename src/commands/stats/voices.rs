//! `grans stats speakers`: who was heard in meetings with speaker names.

use chrono::FixedOffset;
use colored::Colorize;

use super::{day, percent};
use crate::stats::speakers::SpeakersReport;

/// How many series to list.
const SERIES_LISTED: usize = 10;

pub(super) fn print(report: &SpeakersReport, limit: usize, tz: &FixedOffset) {
    let Some(since) = report.since else {
        println!("No meetings with speaker names found.");
        return;
    };
    println!(
        "{} meetings with speaker names, since {}.",
        report.meetings_with_names,
        day(since, tz)
    );
    println!();
    println!(
        "{}",
        format!(
            "{:>5}  {:>6}  {:>7}  {:>11}  {:>11}  {:>8}  {:>8}  speaker",
            "heard", "share", "invited", "not heard", "uninvited", "you rep.", "rep. you"
        )
        .dimmed()
    );
    let shown = if limit == 0 {
        report.people.len()
    } else {
        limit
    };
    for p in report.people.iter().take(shown) {
        println!(
            "{:>5}  {:>6}  {:>7}  {:>11}  {:>11}  {:>8}  {:>8}  {}",
            p.spoke_in,
            p.share_when_heard.map_or("-".to_string(), percent),
            p.invited_to,
            p.invited_not_heard,
            p.heard_not_invited,
            p.you_answered,
            p.answered_you,
            p.name.bold()
        );
    }
    let hidden = report.people.len().saturating_sub(shown);
    if hidden > 0 {
        println!(
            "{}",
            format!("... and {hidden} more (--limit 0 shows all)").dimmed()
        );
    }
    println!(
        "{}",
        "heard: meetings they spoke in; share: of all talk in those meetings.\n\
         invited: meetings they were invited to; not heard: of those, meetings\n\
         they never spoke in; uninvited: meetings they spoke in without an\n\
         invitation. you rep.: times you spoke right after them; rep. you: times\n\
         they spoke right after you."
            .dimmed()
    );

    if !report.series.is_empty() {
        println!();
        println!("{}", "Who talks most in each series".bold());
        for s in report.series.iter().take(SERIES_LISTED) {
            println!(
                "{:>4} meetings  {:>4}  {:<24}  {}",
                s.meetings,
                percent(s.top_share),
                s.top_speaker,
                s.title.bold()
            );
        }
    }

    println!();
    println!("{}", "How evenly the talk is spread, by month".bold());
    for e in &report.evenness {
        println!(
            "{}  {:>4} meetings  {}",
            e.month.format("%Y-%m"),
            e.meetings,
            e.typical_evenness
                .map_or("-".to_string(), |v| format!("{v:.2}"))
        );
    }
    println!(
        "{}",
        "evenness: 1 when you and every named speaker talked equally, near 0 when\n\
         one voice did nearly all of it; the median meeting of the month."
            .dimmed()
    );
}
