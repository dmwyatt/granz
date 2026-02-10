use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::output::format::OutputMode;

pub fn list(conn: &Connection, visibility: Option<&str>, mode: OutputMode) -> Result<()> {
    let recipes = crate::db::recipes::list_recipes(conn, visibility)?;

    let refs: Vec<_> = recipes.iter().collect();

    match mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_recipes(&refs));
        }
        OutputMode::Tty => {
            if recipes.is_empty() {
                println!("No recipes found.");
                return Ok(());
            }
            for recipe in &recipes {
                println!("{}", crate::output::table::format_recipe_row(recipe));
            }
        }
    }

    Ok(())
}

pub fn show(conn: &Connection, query: &str, mode: OutputMode) -> Result<()> {
    let found = crate::db::recipes::show_recipe(conn, query)?;

    match found {
        None => bail!("No recipe found matching \"{}\"", query),
        Some(recipe) => {
            match mode {
                OutputMode::Json => {
                    println!("{}", crate::output::json::to_json(&recipe));
                }
                OutputMode::Tty => {
                    println!("{}", crate::output::table::format_recipe_row(&recipe));
                    if let Some(config) = &recipe.config {
                        if let Some(desc) = &config.description {
                            println!("\n{}", desc);
                        }
                        if let Some(instructions) = &config.instructions {
                            println!("\nInstructions:");
                            for line in instructions.lines().take(20) {
                                println!("  {}", line);
                            }
                        }
                    }
                }
            }
            Ok(())
        }
    }
}
