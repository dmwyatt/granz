use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Utc};
use rusqlite::Connection;
use serde::Serialize;

mod conversation;
mod load;
mod people;
mod recurring;
mod summary;
mod vocabulary;
mod voices;

use crate::cli::args::{StatsAction, StatsPeriod};
use crate::cli::context::RunContext;
use crate::db::stats::{for_each_transcript, for_each_utterance_text, meeting_records};
use crate::output::format::{OutputMode, format_number};
use crate::output::json::to_json;
use crate::query::dates::{DateRange, build_date_range};
use crate::stats::overview::overview;
use crate::stats::page::{self, Payload, WordsPayload};
use crate::stats::words::WordCounter;
use crate::stats::{MeetingRow, series, summarize, time};

pub fn run(
    conn: &Connection,
    action: Option<&StatsAction>,
    html: Option<&Path>,
    period: &StatsPeriod,
    ctx: &RunContext,
) -> Result<()> {
    let range = period_range(period, ctx);
    match (html, action) {
        (Some(_), Some(_)) => {
            bail!("--html writes the whole page and does not take a subcommand")
        }
        (Some(path), None) => write_page(conn, path, range.as_ref(), ctx),
        (None, Some(StatsAction::Words { terms })) => {
            let rows = meeting_rows(conn, range.as_ref())?;
            let counter = count_words(conn, range.as_ref(), terms, &rows, ctx)?;
            emit(ctx, &counter.report(&rows), |r| {
                vocabulary::print(r, &ctx.tz)
            });
            Ok(())
        }
        (None, action) => {
            let rows = meeting_rows(conn, range.as_ref())?;
            report_on_rows(action, &rows, ctx);
            Ok(())
        }
    }
}

/// Print `report` as JSON, or with `print` for a terminal.
fn emit<T: Serialize>(ctx: &RunContext, report: &T, print: impl FnOnce(&T)) {
    match ctx.output_mode {
        OutputMode::Json => println!("{}", to_json(report)),
        OutputMode::Tty => print(report),
    }
}

/// The statistics built from the meeting rows alone.
fn report_on_rows(action: Option<&StatsAction>, rows: &[MeetingRow], ctx: &RunContext) {
    let tz = &ctx.tz;
    match action {
        None => emit(ctx, &overview(rows), |o| summary::print_overview(o, tz)),
        Some(StatsAction::Meetings) => emit(ctx, &rows, |r| summary::print_meetings(r, tz)),
        Some(StatsAction::Time { by }) => {
            emit(ctx, &time::report(rows, *by, tz), |r| load::print(r, *by))
        }
        Some(StatsAction::Series { limit }) => {
            let mut report = series::report(rows);
            truncate(&mut report.series, *limit, ctx);
            emit(ctx, &report, |r| recurring::print(r, *limit, tz));
        }
        Some(StatsAction::People { limit }) => {
            let mut report = crate::stats::people::report(rows);
            truncate(&mut report.people, *limit, ctx);
            emit(ctx, &report, |r| people::print(r, *limit, tz));
        }
        Some(StatsAction::Talk) => emit(ctx, &crate::stats::talk::report(rows, tz), |r| {
            conversation::print(r, tz)
        }),
        Some(StatsAction::Speakers { limit }) => {
            let mut report = crate::stats::speakers::report(rows, tz);
            truncate(&mut report.people, *limit, ctx);
            emit(ctx, &report, |r| voices::print(r, *limit, tz));
        }
        Some(StatsAction::Words { .. }) => unreachable!("words read text, not rows"),
    }
}

/// `--limit` cuts JSON lists to the same length the table shows; the
/// table printers apply it themselves so they can say how many are hidden.
fn truncate<T>(list: &mut Vec<T>, limit: usize, ctx: &RunContext) {
    if ctx.output_mode == OutputMode::Json && limit > 0 {
        list.truncate(limit);
    }
}

/// One pass over the transcript text in `range`, listening for the full
/// names of everyone invited to `rows`.
fn count_words(
    conn: &Connection,
    range: Option<&DateRange>,
    terms: &[String],
    rows: &[MeetingRow],
    ctx: &RunContext,
) -> Result<WordCounter> {
    let people: Vec<(String, String)> = crate::stats::people::names(rows).into_iter().collect();
    let mut counter = WordCounter::new(ctx.tz, terms, &people);
    for_each_utterance_text(conn, range, |meeting, created_at, mine, text| {
        counter.add(meeting, created_at, mine, text)
    })?;
    Ok(counter)
}

/// Write the stats page for the meetings in `period` to `path`.
fn write_page(
    conn: &Connection,
    path: &Path,
    range: Option<&DateRange>,
    ctx: &RunContext,
) -> Result<()> {
    let rows = meeting_rows(conn, range)?;
    let words = count_words(conn, range, &[], &rows, ctx)?;
    let html = page::render(&Payload {
        generated_at: Utc::now(),
        meetings: &rows,
        words: WordsPayload::new(&words, &rows),
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

/// A local date, as the tables print it.
fn day(at: DateTime<Utc>, tz: &FixedOffset) -> String {
    at.with_timezone(tz).format("%Y-%m-%d").to_string()
}

/// A 0-1 fraction as a whole percentage.
fn percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}
