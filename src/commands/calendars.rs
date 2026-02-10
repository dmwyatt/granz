use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::output::format::OutputMode;
use crate::query::dates::build_date_range;

pub fn list(conn: &Connection, mode: OutputMode) -> Result<()> {
    let calendars = crate::db::calendars::list_calendars(conn)?;

    let refs: Vec<_> = calendars.iter().collect();

    match mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_calendars(&refs));
        }
        OutputMode::Tty => {
            if calendars.is_empty() {
                println!("No calendars found.");
                return Ok(());
            }
            for cal in &calendars {
                println!("{}", crate::output::table::format_calendar_row(cal));
            }
        }
    }

    Ok(())
}

pub fn events(
    conn: &Connection,
    calendar_filter: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    date: Option<&str>,
    ctx: &RunContext,
) -> Result<()> {
    let date_range = build_date_range(from, to, date, Utc::now(), &ctx.tz);
    let events = crate::db::calendars::list_events(conn, calendar_filter, date_range.as_ref())?;

    let refs: Vec<_> = events.iter().collect();

    match ctx.output_mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_events(&refs));
        }
        OutputMode::Tty => {
            if events.is_empty() {
                println!("No events found.");
                return Ok(());
            }
            for event in &events {
                println!("{}", crate::output::table::format_event_row(event, &ctx.tz));
            }
        }
    }

    Ok(())
}
