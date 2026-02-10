//! Sync data from the Granola API into the local database.
//!
//! This module implements the `grans sync` command which fetches data from
//! the Granola API and upserts it into the local SQLite database.

use anyhow::Result;
use log::debug;
use rusqlite::Connection;

use crate::api::ApiClient;
use crate::cli::args::SyncAction;
use crate::db::sync::{
    self, upsert_calendar_events, upsert_calendars_from_selection, upsert_documents, upsert_people,
    upsert_recipes, upsert_templates, SyncStats,
};
use crate::output::format::OutputMode;
use crate::output::progress::create_spinner;

use super::sync_panels::sync_panels;
use super::sync_transcripts::sync_transcripts;

/// Run the sync command
pub fn run(
    conn: &Connection,
    action: &Option<SyncAction>,
    dry_run: bool,
    token: Option<&str>,
    mode: OutputMode,
) -> Result<()> {
    match action {
        None => {
            // Full sync: all entity types
            sync_all(conn, dry_run, token, mode)
        }
        Some(SyncAction::Documents) => sync_documents(conn, dry_run, token, mode),
        Some(SyncAction::Transcripts {
            limit,
            since,
            delay_ms,
            retry,
            embed,
        }) => {
            let result = sync_transcripts(
                conn,
                *limit,
                since.as_deref(),
                *delay_ms,
                *retry,
                dry_run,
                token,
                mode,
            );

            // If sync succeeded and --embed was requested, build embeddings
            if result.is_ok() && *embed && !dry_run {
                crate::commands::embed::run_after_sync(conn, mode)?;
            }

            result
        }
        Some(SyncAction::People) => sync_people(conn, dry_run, token, mode),
        Some(SyncAction::Calendars) => sync_calendars(conn, dry_run, token, mode),
        Some(SyncAction::Templates) => sync_templates(conn, dry_run, token, mode),
        Some(SyncAction::Recipes) => sync_recipes(conn, dry_run, token, mode),
        Some(SyncAction::Panels {
            limit,
            since,
            delay_ms,
            retry,
        }) => sync_panels(
            conn,
            *limit,
            since.as_deref(),
            *delay_ms,
            *retry,
            dry_run,
            token,
            mode,
        ),
    }
}

/// Sync all entity types
fn sync_all(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("Starting full sync (dry_run={})", dry_run);
    eprintln!("[grans] Starting full sync from Granola API...");

    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    // Sync each entity type
    let mut total_stats = FullSyncStats::default();

    // Documents
    eprintln!("[grans] Syncing documents...");
    match sync_documents_with_client(conn, &client, dry_run) {
        Ok(stats) => {
            total_stats.documents = stats;
            eprintln!(
                "[grans] Documents: {} inserted, {} updated, {} unchanged",
                stats.inserted, stats.updated, stats.unchanged
            );
        }
        Err(e) => eprintln!("[grans] Documents sync failed: {}", e),
    }

    // People
    eprintln!("[grans] Syncing people...");
    match sync_people_with_client(conn, &client, dry_run) {
        Ok(stats) => {
            total_stats.people = stats;
            eprintln!(
                "[grans] People: {} inserted, {} updated",
                stats.inserted, stats.updated
            );
        }
        Err(e) => eprintln!("[grans] People sync failed: {}", e),
    }

    // Calendar events
    eprintln!("[grans] Syncing calendar events...");
    match sync_calendars_with_client(conn, &client, dry_run) {
        Ok(stats) => {
            total_stats.events = stats;
            eprintln!(
                "[grans] Events: {} inserted, {} updated",
                stats.inserted, stats.updated
            );
        }
        Err(e) => eprintln!("[grans] Calendar sync failed: {}", e),
    }

    // Templates
    eprintln!("[grans] Syncing templates...");
    match sync_templates_with_client(conn, &client, dry_run) {
        Ok(stats) => {
            total_stats.templates = stats;
            eprintln!(
                "[grans] Templates: {} inserted, {} updated, {} unchanged",
                stats.inserted, stats.updated, stats.unchanged
            );
        }
        Err(e) => eprintln!("[grans] Templates sync failed: {}", e),
    }

    // Recipes
    eprintln!("[grans] Syncing recipes...");
    match sync_recipes_with_client(conn, &client, dry_run) {
        Ok(stats) => {
            total_stats.recipes = stats;
            eprintln!(
                "[grans] Recipes: {} inserted, {} updated, {} unchanged",
                stats.inserted, stats.updated, stats.unchanged
            );
        }
        Err(e) => eprintln!("[grans] Recipes sync failed: {}", e),
    }

    // Print summary
    print_full_sync_summary(&total_stats, dry_run, mode);

    Ok(())
}

#[derive(Default)]
struct FullSyncStats {
    documents: SyncStats,
    people: SyncStats,
    events: SyncStats,
    templates: SyncStats,
    recipes: SyncStats,
}

fn print_full_sync_summary(stats: &FullSyncStats, dry_run: bool, mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "action": "sync",
                    "dry_run": dry_run,
                    "documents": {
                        "inserted": stats.documents.inserted,
                        "updated": stats.documents.updated,
                        "unchanged": stats.documents.unchanged,
                    },
                    "people": {
                        "inserted": stats.people.inserted,
                        "updated": stats.people.updated,
                    },
                    "events": {
                        "inserted": stats.events.inserted,
                        "updated": stats.events.updated,
                    },
                    "templates": {
                        "inserted": stats.templates.inserted,
                        "updated": stats.templates.updated,
                        "unchanged": stats.templates.unchanged,
                    },
                    "recipes": {
                        "inserted": stats.recipes.inserted,
                        "updated": stats.recipes.updated,
                        "unchanged": stats.recipes.unchanged,
                    },
                })
            );
        }
        _ => {
            let prefix = if dry_run { "[dry-run] " } else { "" };
            println!();
            println!("{}Sync complete:", prefix);
            println!(
                "  Documents:  {} inserted, {} updated, {} unchanged",
                stats.documents.inserted, stats.documents.updated, stats.documents.unchanged
            );
            println!(
                "  People:     {} inserted, {} updated",
                stats.people.inserted, stats.people.updated
            );
            println!(
                "  Events:     {} inserted, {} updated",
                stats.events.inserted, stats.events.updated
            );
            println!(
                "  Templates:  {} inserted, {} updated, {} unchanged",
                stats.templates.inserted, stats.templates.updated, stats.templates.unchanged
            );
            println!(
                "  Recipes:    {} inserted, {} updated, {} unchanged",
                stats.recipes.inserted, stats.recipes.updated, stats.recipes.unchanged
            );
        }
    }
}

// ============================================================================
// Individual sync functions
// ============================================================================

fn sync_documents(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("sync_documents (dry_run={})", dry_run);
    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    let spinner = create_spinner("Fetching documents from API...");
    let documents = client.get_documents()?;
    spinner.finish_and_clear();
    debug!("Fetched {} documents from API", documents.len());
    eprintln!("[grans] Fetched {} documents", documents.len());

    let stats = if dry_run {
        SyncStats {
            inserted: documents.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        }
    } else {
        upsert_documents(conn, &documents)?
    };

    print_sync_stats("documents", &stats, dry_run, mode);

    if !dry_run {
        sync::set_last_sync_time(conn, "documents")?;
    }

    Ok(())
}

fn sync_documents_with_client(
    conn: &Connection,
    client: &ApiClient,
    dry_run: bool,
) -> Result<SyncStats> {
    let documents = client.get_documents()?;

    if dry_run {
        return Ok(SyncStats {
            inserted: documents.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        });
    }

    upsert_documents(conn, &documents)
}

fn sync_people(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("sync_people (dry_run={})", dry_run);
    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    let spinner = create_spinner("Fetching people from API...");
    let people = client.get_people()?;
    spinner.finish_and_clear();
    debug!("Fetched {} people from API", people.len());
    eprintln!("[grans] Fetched {} people", people.len());

    let stats = if dry_run {
        SyncStats {
            inserted: people.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        }
    } else {
        upsert_people(conn, &people)?
    };

    print_sync_stats("people", &stats, dry_run, mode);

    if !dry_run {
        sync::set_last_sync_time(conn, "people")?;
    }

    Ok(())
}

fn sync_people_with_client(
    conn: &Connection,
    client: &ApiClient,
    dry_run: bool,
) -> Result<SyncStats> {
    let people = client.get_people()?;

    if dry_run {
        return Ok(SyncStats {
            inserted: people.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        });
    }

    upsert_people(conn, &people)
}

fn sync_calendars(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("sync_calendars (dry_run={})", dry_run);
    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    let spinner = create_spinner("Fetching calendar events from API...");
    let events = client.refresh_calendar_events()?;
    spinner.finish_and_clear();
    debug!("Fetched {} calendar events from API", events.len());
    eprintln!("[grans] Fetched {} calendar events", events.len());

    // Also fetch calendar selection info
    if let Ok(selected) = client.get_selected_calendars() {
        if let Some(calendars_selected) = selected.calendars_selected {
            let enabled = selected.enabled_calendars.unwrap_or_default();
            if !dry_run {
                upsert_calendars_from_selection(conn, &calendars_selected, &enabled)?;
            }
        }
    }

    let stats = if dry_run {
        SyncStats {
            inserted: events.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        }
    } else {
        upsert_calendar_events(conn, &events)?
    };

    print_sync_stats("calendar events", &stats, dry_run, mode);

    if !dry_run {
        sync::set_last_sync_time(conn, "calendars")?;
    }

    Ok(())
}

fn sync_calendars_with_client(
    conn: &Connection,
    client: &ApiClient,
    dry_run: bool,
) -> Result<SyncStats> {
    // Fetch events
    let events = client.refresh_calendar_events()?;

    // Also fetch calendar selection info
    if let Ok(selected) = client.get_selected_calendars() {
        if let Some(calendars_selected) = selected.calendars_selected {
            let enabled = selected.enabled_calendars.unwrap_or_default();
            if !dry_run {
                upsert_calendars_from_selection(conn, &calendars_selected, &enabled)?;
            }
        }
    }

    if dry_run {
        return Ok(SyncStats {
            inserted: events.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        });
    }

    upsert_calendar_events(conn, &events)
}

fn sync_templates(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("sync_templates (dry_run={})", dry_run);
    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    let spinner = create_spinner("Fetching templates from API...");
    let templates = client.get_templates()?;
    spinner.finish_and_clear();
    debug!("Fetched {} templates from API", templates.len());
    eprintln!("[grans] Fetched {} templates", templates.len());

    let stats = if dry_run {
        SyncStats {
            inserted: templates.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        }
    } else {
        upsert_templates(conn, &templates)?
    };

    print_sync_stats("templates", &stats, dry_run, mode);

    if !dry_run {
        sync::set_last_sync_time(conn, "templates")?;
    }

    Ok(())
}

fn sync_templates_with_client(
    conn: &Connection,
    client: &ApiClient,
    dry_run: bool,
) -> Result<SyncStats> {
    let templates = client.get_templates()?;

    if dry_run {
        return Ok(SyncStats {
            inserted: templates.len(),
            updated: 0,
            unchanged: 0,
            errors: 0,
        });
    }

    upsert_templates(conn, &templates)
}

fn sync_recipes(conn: &Connection, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    debug!("sync_recipes (dry_run={})", dry_run);
    let token = crate::api::resolve_token(token)?;
    let client = ApiClient::new(token)?;

    let spinner = create_spinner("Fetching recipes from API...");
    let response = client.get_recipes()?;
    let total = response.default_recipes.len()
        + response.public_recipes.len()
        + response.user_recipes.len()
        + response.shared_recipes.len()
        + response.unlisted_recipes.len();
    spinner.finish_and_clear();
    eprintln!("[grans] Fetched {} recipes", total);

    let stats = if dry_run {
        SyncStats {
            inserted: total,
            updated: 0,
            unchanged: 0,
            errors: 0,
        }
    } else {
        upsert_recipes(conn, &response)?
    };

    print_sync_stats("recipes", &stats, dry_run, mode);

    if !dry_run {
        sync::set_last_sync_time(conn, "recipes")?;
    }

    Ok(())
}

fn sync_recipes_with_client(
    conn: &Connection,
    client: &ApiClient,
    dry_run: bool,
) -> Result<SyncStats> {
    let response = client.get_recipes()?;

    let total = response.default_recipes.len()
        + response.public_recipes.len()
        + response.user_recipes.len()
        + response.shared_recipes.len()
        + response.unlisted_recipes.len();

    if dry_run {
        return Ok(SyncStats {
            inserted: total,
            updated: 0,
            unchanged: 0,
            errors: 0,
        });
    }

    upsert_recipes(conn, &response)
}

// ============================================================================
// Output helpers
// ============================================================================

fn print_sync_stats(entity: &str, stats: &SyncStats, dry_run: bool, mode: OutputMode) {
    match mode {
        OutputMode::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "action": format!("sync_{}", entity.replace(' ', "_")),
                    "dry_run": dry_run,
                    "inserted": stats.inserted,
                    "updated": stats.updated,
                    "unchanged": stats.unchanged,
                    "errors": stats.errors,
                })
            );
        }
        _ => {
            let prefix = if dry_run { "[dry-run] " } else { "" };
            println!(
                "{}Sync {}: {} inserted, {} updated, {} unchanged",
                prefix, entity, stats.inserted, stats.updated, stats.unchanged
            );
        }
    }
}
