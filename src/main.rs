mod api;
mod cli;
mod commands;
mod db;
mod embed;
mod models;
mod output;
mod platform;
mod query;
mod sync;
mod tiptap;
mod update;

use anyhow::Result;
use clap::Parser;
use rusqlite::Connection;

use cli::args::{AdminAction, Cli, Commands};
use cli::context::RunContext;
use commands::search::SearchMode;

fn main() -> Result<()> {
    setup_broken_pipe_handling();
    let cli = Cli::parse();
    init_logging(cli.verbose);

    // Update command doesn't need a database
    if let Commands::Update {
        check,
        use_gh_auth,
        wait,
        timeout,
    } = &cli.command
    {
        commands::update::run(*check, *use_gh_auth, *wait, *timeout)?;
        return Ok(());
    }

    let ctx = RunContext::from_args(cli.json, cli.no_color, cli.utc)?;

    // Admin DB commands don't need a database connection
    if let Commands::Admin {
        action: AdminAction::Db { action },
    } = &cli.command
    {
        let db_path = cli.db.as_deref().map(|p| p.to_path_buf()).unwrap_or_else(|| {
            db::connection::default_db_path().expect("Failed to get default db path")
        });
        commands::db::run_with_path(action, &db_path)?;
        return Ok(());
    }

    // Dropbox commands (init, push, pull, status, logout)
    if let Commands::Dropbox { action } = &cli.command {
        commands::sync::run_dropbox(action, ctx.output_mode, &ctx.tz)?;
        return Ok(());
    }

    // Sync command (from Granola API)
    if let Commands::Sync { action, dry_run } = &cli.command {
        let conn = get_connection(cli.db.as_deref())?;
        commands::sync_granola::run(&conn, action, *dry_run, cli.token.as_deref(), ctx.output_mode)?;
        return Ok(());
    }

    // Embed command
    if let Commands::Embed { action, yes, batch_size } = &cli.command {
        let conn = get_connection(cli.db.as_deref())?;
        commands::embed::run(&conn, action.as_ref(), *yes, *batch_size, ctx.output_mode)?;
        return Ok(());
    }

    // Benchmark command
    if let Commands::Benchmark { action } = &cli.command {
        let conn = get_connection(cli.db.as_deref())?;
        commands::benchmark::run(&conn, action, ctx.output_mode)?;
        return Ok(());
    }

    // Get database connection
    let conn = get_connection(cli.db.as_deref())?;

    match &cli.command {
        // === Daily Use Commands ===
        Commands::Search {
            query,
            r#in,
            semantic,
            context,
            meeting,
            from,
            to,
            date,
            speaker,
            yes,
            limit,
            include_deleted,
        } => {
            let mode = SearchMode::from_cli_args(
                *semantic,
                *context,
                r#in,
                meeting.as_deref(),
                speaker.as_ref(),
                *yes,
                *limit,
            );
            let date_range = query::dates::build_date_range(
                from.as_deref(),
                to.as_deref(),
                date.as_deref(),
                chrono::Utc::now(),
                &ctx.tz,
            );
            commands::search::search(&conn, query, mode, date_range, *include_deleted, &ctx)?;
        }

        Commands::List {
            person,
            from,
            to,
            date,
            include_deleted,
        } => {
            commands::meetings::list(
                &conn,
                person.as_deref(),
                from.as_deref(),
                to.as_deref(),
                date.as_deref(),
                *include_deleted,
                &ctx,
            )?;
        }

        Commands::Show {
            meeting,
            transcript,
            notes,
            speaker,
        } => {
            commands::meetings::show(&conn, meeting, *transcript, *notes, speaker.as_ref(), &ctx)?;
        }

        Commands::With {
            person,
            from,
            to,
            date,
            include_deleted,
        } => {
            commands::meetings::with_person(
                &conn,
                person,
                from.as_deref(),
                to.as_deref(),
                date.as_deref(),
                *include_deleted,
                &ctx,
            )?;
        }

        Commands::Recent => {
            commands::meetings::list(
                &conn,
                None,
                None,
                None,
                Some("this-week"),
                false,
                &ctx,
            )?;
        }

        Commands::Today => {
            commands::meetings::list(&conn, None, None, None, Some("today"), false, &ctx)?;
        }

        Commands::Info => {
            let db_path = cli.db.as_deref().map(|p| p.to_path_buf()).unwrap_or_else(|| {
                db::connection::default_db_path().expect("Failed to get default db path")
            });
            commands::info::run(&conn, &db_path, &ctx)?;
        }

        Commands::Benchmark { .. } => unreachable!(), // Handled above
        Commands::Dropbox { .. } => unreachable!(), // Handled above
        Commands::Update { .. } => unreachable!(), // Handled above
        Commands::Sync { .. } => unreachable!(), // Handled above
        Commands::Embed { .. } => unreachable!(), // Handled above

        // === Browse Commands ===
        Commands::Browse { action } => {
            commands::browse::run(&conn, action, &ctx)?;
        }

        // === Admin Commands ===
        Commands::Admin { action } => match action {
            AdminAction::Db { .. } => unreachable!(), // Handled above
            AdminAction::Transcripts { action } => {
                commands::transcripts::run_admin(&conn, action, cli.token.as_deref(), ctx.output_mode)?;
            }
            AdminAction::Token { clipboard } => {
                let token = api::get_auth_token()?;
                if *clipboard {
                    platform::copy_to_clipboard(&token)?;
                    eprintln!("Token copied to clipboard.");
                } else {
                    println!("{}", token);
                }
            }
        },
    }

    Ok(())
}

/// Initialize logging based on the `--verbose` flag or `GRANS_LOG` env var.
///
/// - `GRANS_LOG` env var: full filter control (e.g. `GRANS_LOG=grans::api=trace`)
/// - `--verbose`: sets `grans` crate to `Debug` level
/// - Otherwise: `Warn` level only (effectively silent)
fn init_logging(verbose: bool) {
    let env_var = std::env::var("GRANS_LOG").ok();

    let mut builder = env_logger::Builder::new();
    builder.format_target(true);
    builder.format_module_path(false);

    if let Some(ref filter) = env_var {
        builder.parse_filters(filter);
    } else if verbose {
        builder.filter_module("grans", log::LevelFilter::Debug);
    } else {
        builder.filter_level(log::LevelFilter::Warn);
    }

    builder.init();
}

/// Handle broken pipe gracefully instead of panicking.
///
/// When output is piped to a process that exits early (e.g., `grans list --json | head -1`),
/// Rust's `println!` panics because the runtime sets SIGPIPE to SIG_IGN. This function:
/// - On Unix: resets SIGPIPE to default behavior so the OS terminates the process cleanly
/// - On all platforms: installs a panic hook that exits silently on stdout pipe failures,
///   as a fallback (and the primary handler on Windows where there's no SIGPIPE)
fn setup_broken_pipe_handling() {
    #[cfg(unix)]
    unsafe {
        // SIGPIPE = 13, SIG_DFL = 0 (POSIX constants, stable across all Unix platforms)
        unsafe extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        signal(13, 0);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");

        if msg.contains("failed printing to stdout") {
            std::process::exit(0);
        }

        default_hook(info);
    }));
}

/// Get a database connection, optionally at a specific path
fn get_connection(db_path: Option<&std::path::Path>) -> Result<Connection> {
    match db_path {
        Some(path) => db::connection::open_db_at_path(path),
        None => db::connection::open_or_create_db(),
    }
}
