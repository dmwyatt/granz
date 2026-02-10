use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::db::info::{get_info_db_only, DbInfo};
use crate::output::format::OutputMode;

/// Run info command
pub fn run(conn: &Connection, db_path: &Path, ctx: &RunContext) -> Result<()> {
    let info = get_info_db_only(conn, db_path)?;

    match ctx.output_mode {
        OutputMode::Json => print_json(&info),
        OutputMode::Tty => print_tty(&info, &ctx.tz),
    }

    Ok(())
}

fn print_json(info: &DbInfo) {
    println!("{}", serde_json::to_string_pretty(info).unwrap());
}

fn print_tty(info: &DbInfo, tz: &FixedOffset) {
    println!("\x1b[1mContent\x1b[0m");
    println!("\x1b[2m───────\x1b[0m");

    println!(
        "Documents:          \x1b[1m{}\x1b[0m",
        format_number(info.total_documents)
    );

    let transcript_pct = if info.total_documents > 0 {
        (info.documents_with_transcripts as f64 / info.total_documents as f64) * 100.0
    } else {
        0.0
    };
    println!(
        "  With Transcripts: {} ({:.1}%)",
        format_number(info.documents_with_transcripts),
        transcript_pct
    );

    println!(
        "  Without:          {}",
        format_number(info.documents_without_transcripts)
    );

    if let (Some(earliest), Some(latest)) = (&info.earliest_document, &info.latest_document) {
        println!(
            "  Date Range:       {} to {}",
            format_date(earliest, tz),
            format_date(latest, tz)
        );
    }

    println!("People:             {}", format_number(info.total_people));
    println!("Calendars:          {}", format_number(info.total_calendars));
    println!("Events:             {}", format_number(info.total_events));
    println!("Templates:          {}", format_number(info.total_templates));
    println!("Recipes:            {}", format_number(info.total_recipes));
    println!("Panels:             {}", format_number(info.total_panels));
    println!("Utterances:         {}", format_number(info.total_utterances));

    if info.total_chunks > 0 {
        if let Some(stats) = &info.chunk_size_stats {
            println!(
                "Chunks:             {} (avg {} chars, range {}-{})",
                format_number(info.total_chunks),
                format_number(stats.avg_chars.round() as i64),
                format_number(stats.min_chars as i64),
                format_number(stats.max_chars as i64)
            );
        } else {
            println!("Chunks:             {}", format_number(info.total_chunks));
        }
    }

    println!();
    println!("\x1b[1mDatabase\x1b[0m");
    println!("\x1b[2m────────\x1b[0m");

    println!(
        "Path:           {}",
        info.db_path.display()
    );
    println!(
        "Size:           {}",
        format_size(info.db_size_bytes)
    );
    println!(
        "Schema version: {}",
        info.schema_version
    );
}

fn format_number(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_date(iso_date: &str, tz: &FixedOffset) -> String {
    // Parse ISO 8601 date and format as YYYY-MM-DD in user's timezone
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso_date) {
        dt.with_timezone(tz).format("%Y-%m-%d").to_string()
    } else {
        // Fallback: try to extract just the date part
        iso_date.split('T').next().unwrap_or(iso_date).to_string()
    }
}
