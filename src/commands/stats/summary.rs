//! The overview and the per-meeting table.

use chrono::FixedOffset;
use colored::Colorize;

use super::{day, percent};
use crate::output::format::format_number;
use crate::stats::MeetingRow;
use crate::stats::overview::{Overview, share};

pub(super) fn print_overview(o: &Overview, tz: &FixedOffset) {
    if o.meetings == 0 {
        println!("No meetings found.");
        return;
    }
    println!(
        "Meetings:       {} ({} with a transcript)",
        format_number(o.meetings as i64).bold(),
        format_number(o.with_transcript as i64)
    );
    if let (Some(first), Some(last)) = (o.first_meeting, o.last_meeting) {
        println!("Date range:     {} to {}", day(first, tz), day(last, tz));
    }
    println!(
        "Spoken time:    {} hours",
        format_number(o.spoken_hours.round() as i64)
    );
    if let Some(mine) = o.my_share {
        println!("Your share:     {} of what was said", percent(mine));
    }
    match o.speaker_names_since {
        Some(since) => println!(
            "Speaker names:  since {} ({} meetings)",
            day(since, tz),
            format_number(o.meetings_with_speaker_names as i64)
        ),
        None => println!("Speaker names:  none detected"),
    }
}

pub(super) fn print_meetings(rows: &[MeetingRow], tz: &FixedOffset) {
    if rows.is_empty() {
        println!("No meetings found.");
        return;
    }
    println!(
        "{}",
        format!(
            "{:<10}  {:>8}  {:>4}  {:>7}  {:>5}  title",
            "date", "spoken", "you", "invited", "named"
        )
        .dimmed()
    );
    for row in rows {
        let (spoken, mine, named) = match &row.transcript {
            Some(t) => (
                format!("{} min", t.span().num_minutes()),
                share(t.my_chars(), t.others_chars()).map_or("-".to_string(), percent),
                t.named_speakers().to_string(),
            ),
            None => ("-".to_string(), "-".to_string(), "-".to_string()),
        };
        println!(
            "{:<10}  {:>8}  {:>4}  {:>7}  {:>5}  {}",
            day(row.created_at, tz),
            spoken,
            mine,
            row.invited(),
            named,
            row.title.bold()
        );
    }
}
