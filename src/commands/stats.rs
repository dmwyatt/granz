use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Utc};
use colored::Colorize;
use rusqlite::Connection;

use crate::cli::args::{StatsAction, StatsPeriod};
use crate::cli::context::RunContext;
use crate::db::stats::{for_each_transcript, meeting_records};
use crate::output::format::{OutputMode, format_number};
use crate::output::json::to_json;
use crate::query::dates::{DateRange, build_date_range};
use crate::stats::overview::{Overview, overview, share};
use crate::stats::page::{self, Payload};
use crate::stats::{MeetingRow, summarize};

pub fn run(
    conn: &Connection,
    action: Option<&StatsAction>,
    html: Option<&Path>,
    period: &StatsPeriod,
    ctx: &RunContext,
) -> Result<()> {
    if let Some(path) = html {
        if action.is_some() {
            bail!("--html writes the whole page and does not take a subcommand");
        }
        return write_page(conn, path, period, ctx);
    }
    let rows = meeting_rows(conn, period_range(period, ctx).as_ref())?;

    match (action, ctx.output_mode) {
        (None, OutputMode::Json) => println!("{}", to_json(&overview(&rows))),
        (None, OutputMode::Tty) => print_overview(&overview(&rows), &ctx.tz),
        (Some(StatsAction::Meetings), OutputMode::Json) => println!("{}", to_json(&rows)),
        (Some(StatsAction::Meetings), OutputMode::Tty) => print_meetings(&rows, &ctx.tz),
    }
    Ok(())
}

/// Write the stats page for the meetings in `period` to `path`.
fn write_page(
    conn: &Connection,
    path: &Path,
    period: &StatsPeriod,
    ctx: &RunContext,
) -> Result<()> {
    let rows = meeting_rows(conn, period_range(period, ctx).as_ref())?;
    let html = page::render(&Payload {
        generated_at: Utc::now(),
        meetings: &rows,
    })?;
    std::fs::write(path, html).with_context(|| format!("writing {}", path.display()))?;
    match ctx.output_mode {
        OutputMode::Json => println!("{}", to_json(&serde_json::json!({ "path": path }))),
        OutputMode::Tty => println!(
            "Wrote {} meetings to {}",
            format_number(rows.len() as i64),
            path.display()
        ),
    }
    Ok(())
}

fn period_range(period: &StatsPeriod, ctx: &RunContext) -> Option<DateRange> {
    build_date_range(
        period.from.as_deref(),
        period.to.as_deref(),
        period.date.as_deref(),
        Utc::now(),
        &ctx.tz,
    )
}

/// Every meeting in `range` with its transcript summarized, oldest first.
pub fn meeting_rows(conn: &Connection, range: Option<&DateRange>) -> Result<Vec<MeetingRow>> {
    let mut transcripts = HashMap::new();
    for_each_transcript(conn, range, |id, utterances| {
        transcripts.insert(id.to_string(), summarize(utterances));
    })?;
    Ok(meeting_records(conn, range)?
        .into_iter()
        .map(|record| {
            let transcript = transcripts.remove(&record.id).flatten();
            MeetingRow::new(record, transcript)
        })
        .collect())
}

fn print_overview(o: &Overview, tz: &FixedOffset) {
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

fn print_meetings(rows: &[MeetingRow], tz: &FixedOffset) {
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

fn day(at: DateTime<Utc>, tz: &FixedOffset) -> String {
    at.with_timezone(tz).format("%Y-%m-%d").to_string()
}

fn percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}
