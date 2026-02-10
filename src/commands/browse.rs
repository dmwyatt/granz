//! Browse command implementations - entity exploration subcommands.

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::{BrowseAction, CalendarsAction, PeopleAction, RecipesAction, TemplatesAction};
use crate::cli::context::RunContext;
use crate::output::format::OutputMode;

pub fn run(conn: &Connection, action: &BrowseAction, ctx: &RunContext) -> Result<()> {
    match action {
        BrowseAction::People { action } => run_people(conn, action, ctx.output_mode),
        BrowseAction::Calendars { action } => run_calendars(conn, action, ctx),
        BrowseAction::Templates { action } => run_templates(conn, action, ctx.output_mode),
        BrowseAction::Recipes { action } => run_recipes(conn, action, ctx.output_mode),
    }
}

fn run_people(conn: &Connection, action: &PeopleAction, mode: OutputMode) -> Result<()> {
    match action {
        PeopleAction::List { company } => crate::commands::people::list(conn, company.as_deref(), mode),
        PeopleAction::Show { query } => crate::commands::people::show(conn, query, mode),
    }
}

fn run_calendars(conn: &Connection, action: &CalendarsAction, ctx: &RunContext) -> Result<()> {
    match action {
        CalendarsAction::List => crate::commands::calendars::list(conn, ctx.output_mode),
        CalendarsAction::Events {
            calendar,
            from,
            to,
            date,
        } => crate::commands::calendars::events(
            conn,
            calendar.as_deref(),
            from.as_deref(),
            to.as_deref(),
            date.as_deref(),
            ctx,
        ),
    }
}

fn run_templates(conn: &Connection, action: &TemplatesAction, mode: OutputMode) -> Result<()> {
    match action {
        TemplatesAction::List { category } => {
            crate::commands::templates::list(conn, category.as_deref(), mode)
        }
        TemplatesAction::Show { query } => crate::commands::templates::show(conn, query, mode),
    }
}

fn run_recipes(conn: &Connection, action: &RecipesAction, mode: OutputMode) -> Result<()> {
    match action {
        RecipesAction::List { visibility } => {
            crate::commands::recipes::list(conn, visibility.as_deref(), mode)
        }
        RecipesAction::Show { query } => crate::commands::recipes::show(conn, query, mode),
    }
}
