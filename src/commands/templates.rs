use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::output::format::OutputMode;

pub fn list(conn: &Connection, category: Option<&str>, mode: OutputMode) -> Result<()> {
    let templates = crate::db::templates::list_templates(conn, category)?;

    let refs: Vec<_> = templates.iter().collect();

    match mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_templates(&refs));
        }
        OutputMode::Tty => {
            if templates.is_empty() {
                println!("No templates found.");
                return Ok(());
            }
            for tmpl in &templates {
                println!("{}", crate::output::table::format_template_row(tmpl));
            }
        }
    }

    Ok(())
}

pub fn show(conn: &Connection, query: &str, mode: OutputMode) -> Result<()> {
    let found = crate::db::templates::show_template(conn, query)?;

    match found {
        None => bail!("No template found matching \"{}\"", query),
        Some(tmpl) => {
            match mode {
                OutputMode::Json => {
                    println!("{}", crate::output::json::to_json(&tmpl));
                }
                OutputMode::Tty => {
                    println!("{}", crate::output::table::format_template_row(&tmpl));
                    if let Some(desc) = &tmpl.description {
                        println!("\n{}", desc);
                    }
                    if let Some(sections) = &tmpl.sections {
                        println!("\nSections:");
                        for s in sections {
                            let title = s.title.as_deref().unwrap_or("(untitled)");
                            println!("  - {}", title);
                        }
                    }
                }
            }
            Ok(())
        }
    }
}
