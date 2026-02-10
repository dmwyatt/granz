use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::models::SpeakerFilter;
use crate::output::format::OutputMode;
use crate::query::dates::build_date_range;

pub fn list(
    conn: &Connection,
    person: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    date: Option<&str>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    let date_range = build_date_range(from, to, date, Utc::now(), &ctx.tz);
    let docs = crate::db::meetings::list_meetings(conn, person, date_range.as_ref(), include_deleted)?;

    let refs: Vec<_> = docs.iter().collect();

    match ctx.output_mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_meetings(&refs));
        }
        OutputMode::Tty => {
            if docs.is_empty() {
                println!("No meetings found.");
                return Ok(());
            }
            for doc in &docs {
                println!("{}", crate::output::table::format_meeting_row(doc, &ctx.tz));
            }
        }
    }

    Ok(())
}

pub fn show(
    conn: &Connection,
    query: &str,
    transcript_only: bool,
    notes_only: bool,
    speaker: Option<&SpeakerFilter>,
    ctx: &RunContext,
) -> Result<()> {
    let doc = crate::db::meetings::show_meeting(conn, query)?;

    match doc {
        None => bail!("No meeting found matching \"{}\"", query),
        Some(doc) => {
            let doc_id = doc.id.as_deref().unwrap_or("");

            // Handle --notes and/or --transcript flags
            if notes_only || transcript_only {
                let mut output_parts: Vec<String> = vec![];

                // Notes first (if requested)
                if notes_only {
                    let notes = doc.notes_plain.as_deref().unwrap_or("");
                    if notes.is_empty() && !transcript_only {
                        bail!("No notes available for this meeting");
                    }
                    if !notes.is_empty() {
                        output_parts.push(notes.to_string());
                    }
                }

                // Transcript second (if requested)
                if transcript_only {
                    let transcript = filter_by_speaker(
                        crate::db::meetings::get_transcript(conn, doc_id)?,
                        speaker,
                    );
                    if transcript.is_empty() && !notes_only {
                        bail!("No transcript available for this meeting");
                    }
                    if !transcript.is_empty() {
                        let text: String = transcript
                            .iter()
                            .map(|u| crate::output::table::format_utterance(u, false, &ctx.tz))
                            .collect::<Vec<_>>()
                            .join("\n");
                        output_parts.push(text);
                    }
                }

                match ctx.output_mode {
                    OutputMode::Json => {
                        // Build JSON with requested fields
                        let mut obj = serde_json::Map::new();
                        if notes_only {
                            obj.insert(
                                "notes_plain".into(),
                                serde_json::json!(doc.notes_plain),
                            );
                            obj.insert(
                                "notes_markdown".into(),
                                serde_json::json!(doc.notes_markdown),
                            );
                        }
                        if transcript_only {
                            let transcript = filter_by_speaker(
                                crate::db::meetings::get_transcript(conn, doc_id)?,
                                speaker,
                            );
                            obj.insert("transcript".into(), serde_json::json!(transcript));
                        }
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    }
                    _ => {
                        // Plain text output, separator if both
                        println!("{}", output_parts.join("\n\n---\n\n"));
                    }
                }
                return Ok(());
            }

            // Default: existing behavior (show full meeting details)
            let panels = crate::db::panels::load_panels(conn, doc_id)?;

            match ctx.output_mode {
                OutputMode::Json => {
                    let mut detail: serde_json::Value =
                        serde_json::from_str(&crate::output::json::format_meeting_detail(&doc))
                            .unwrap_or_default();
                    if !panels.is_empty() {
                        detail["panels"] = serde_json::json!(panels);
                    }
                    println!("{}", serde_json::to_string_pretty(&detail).unwrap());
                }
                OutputMode::Tty => {
                    println!("{}", crate::output::table::format_meeting_detail(&doc, &ctx.tz));

                    let transcript = filter_by_speaker(
                        crate::db::meetings::get_transcript(conn, doc_id)?,
                        speaker,
                    );
                    if !transcript.is_empty() {
                        println!("\n{}", "Transcript:");
                        for utt in transcript.iter().take(10) {
                            println!("{}", crate::output::table::format_utterance(utt, false, &ctx.tz));
                        }
                        if transcript.len() > 10 {
                            println!("  ... ({} more utterances)", transcript.len() - 10);
                        }
                    }

                    if !panels.is_empty() {
                        use colored::Colorize;
                        println!("\nAI Notes:");
                        for panel in &panels {
                            if let Some(title) = &panel.title {
                                println!("  [{}]", title);
                            }
                            if let Some(md) = &panel.content_markdown {
                                for line in md.lines() {
                                    println!("  {}", line);
                                }
                            }
                            if let Some(url) = &panel.chat_url {
                                println!("  {}", format!("Chat: {}", url).dimmed());
                            }
                            println!();
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn filter_by_speaker(
    utterances: Vec<crate::models::TranscriptUtterance>,
    speaker: Option<&SpeakerFilter>,
) -> Vec<crate::models::TranscriptUtterance> {
    match speaker {
        Some(filter) => utterances
            .into_iter()
            .filter(|u| filter.matches(u.source.as_deref()))
            .collect(),
        None => utterances,
    }
}

/// Show meetings with a specific person (promoted from people meetings)
pub fn with_person(
    conn: &Connection,
    person: &str,
    from: Option<&str>,
    to: Option<&str>,
    date: Option<&str>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    let date_range = build_date_range(from, to, date, Utc::now(), &ctx.tz);
    let matching_docs = crate::db::people::find_meetings_by_person(conn, person, include_deleted)?;

    // Apply date filter if specified
    let matching_docs = if date_range.is_some() {
        let range = date_range.as_ref().unwrap();
        matching_docs
            .into_iter()
            .filter(|doc| {
                if let Some(created_at) = &doc.created_at {
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(created_at) {
                        let dt_utc = dt.with_timezone(&Utc);
                        if let Some(start) = range.start {
                            if dt_utc < start {
                                return false;
                            }
                        }
                        if let Some(end) = range.end {
                            if dt_utc > end {
                                return false;
                            }
                        }
                        return true;
                    }
                }
                false
            })
            .collect()
    } else {
        matching_docs
    };

    let refs: Vec<_> = matching_docs.iter().collect();

    match ctx.output_mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_meetings(&refs));
        }
        OutputMode::Tty => {
            if matching_docs.is_empty() {
                println!("No meetings found with \"{}\".", person);
                return Ok(());
            }
            println!("Meetings with \"{}\":\n", person);
            for doc in &matching_docs {
                println!("{}", crate::output::table::format_meeting_row(doc, &ctx.tz));
            }
        }
    }

    Ok(())
}
