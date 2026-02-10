use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::output::format::OutputMode;

pub fn list(conn: &Connection, company: Option<&str>, mode: OutputMode) -> Result<()> {
    let people = crate::db::people::list_people(conn, company)?;

    let refs: Vec<_> = people.iter().collect();

    match mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_people(&refs));
        }
        OutputMode::Tty => {
            if people.is_empty() {
                println!("No people found.");
                return Ok(());
            }
            for person in &people {
                println!("{}", crate::output::table::format_person_row(person));
            }
        }
    }

    Ok(())
}

pub fn show(conn: &Connection, query: &str, mode: OutputMode) -> Result<()> {
    let matches = crate::db::people::find_person(conn, query)?;

    if matches.is_empty() {
        bail!("No person found matching \"{}\"", query);
    }

    let refs: Vec<_> = matches.iter().collect();

    match mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_people(&refs));
        }
        OutputMode::Tty => {
            for person in &matches {
                println!("{}", crate::output::table::format_person_row(person));
                if let Some(title) = &person.job_title {
                    println!("  Title: {}", title);
                }
                if let Some(company) = &person.company_name {
                    println!("  Company: {}", company);
                }
                println!();
            }
        }
    }

    Ok(())
}
