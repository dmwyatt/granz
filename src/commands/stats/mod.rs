use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Utc};
use rusqlite::Connection;

mod load;
mod recurring;
mod summary;

use crate::cli::args::{StatsAction, StatsPeriod};
use crate::cli::context::RunContext;
use crate::db::stats::{for_each_transcript, meeting_records};
use crate::output::format::{OutputMode, format_number};
use crate::output::json::to_json;
use crate::query::dates::{DateRange, build_date_range};
use crate::stats::overview::overview;
use crate::stats::page::{self, Payload};
use crate::stats::{MeetingRow, series, summarize, time};

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

    let json = ctx.output_mode == OutputMode::Json;
    match action {
        None if json => println!("{}", to_json(&overview(&rows))),
        None => summary::print_overview(&overview(&rows), &ctx.tz),
        Some(StatsAction::Meetings) if json => println!("{}", to_json(&rows)),
        Some(StatsAction::Meetings) => summary::print_meetings(&rows, &ctx.tz),
        Some(StatsAction::Time { by }) => {
            let report = time::report(&rows, *by, &ctx.tz);
            if json {
                println!("{}", to_json(&report));
            } else {
                load::print(&report, *by);
            }
        }
        Some(StatsAction::Series { limit }) => {
            let mut report = series::report(&rows);
            if json {
                if *limit > 0 {
                    report.series.truncate(*limit);
                }
                println!("{}", to_json(&report));
            } else {
                recurring::print(&report, *limit, &ctx.tz);
            }
        }
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

/// A local date, as the tables print it.
fn day(at: DateTime<Utc>, tz: &FixedOffset) -> String {
    at.with_timezone(tz).format("%Y-%m-%d").to_string()
}

/// A 0-1 fraction as a whole percentage.
fn percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}
