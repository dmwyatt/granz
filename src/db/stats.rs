//! The two reads statistics are built on: each meeting's document fields,
//! and its utterances reduced to [`Utterance`]s. JSON columns and timestamps
//! are decoded here, so `stats` works with typed values only.

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use rusqlite::types::ToSql;

use crate::embed::store::blob_to_vector;
use crate::models::DocumentPeople;
use crate::query::dates::DateRange;
use crate::stats::calendar::CalendarInvite;
use crate::stats::notes::PanelRecord;
use crate::stats::{MeetingRecord, Utterance, Voice};

/// Every meeting created in `range`, oldest first. Deleted meetings are
/// left out.
pub fn meeting_records(conn: &Connection, range: Option<&DateRange>) -> Result<Vec<MeetingRecord>> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT id, title, created_at, people_json, google_calendar_event_json,
                coalesce(length(trim(notes_plain, ' ' || char(9, 10, 13))), 0)
         FROM documents d
         WHERE deleted_at IS NULL{filter}
         ORDER BY created_at, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs(&params).as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, u32>(5)?,
        ))
    })?;

    let mut panels = panels(conn, range)?;
    rows.map(|row| {
        let (id, title, created_at, people_json, calendar_json, typed_note_chars) = row?;
        let mut record = decode_record(id, title, created_at, people_json, calendar_json)?;
        record.typed_note_chars = typed_note_chars;
        record.panels = panels.remove(&record.id).unwrap_or_default();
        Ok(record)
    })
    .collect()
}

/// Each meeting's AI summaries that are not deleted, oldest first.
fn panels(
    conn: &Connection,
    range: Option<&DateRange>,
) -> Result<HashMap<String, Vec<PanelRecord>>> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT p.document_id, p.template_slug, coalesce(p.content_markdown, '')
         FROM panels p
         JOIN documents d ON d.id = p.document_id
         WHERE p.deleted_at IS NULL AND d.deleted_at IS NULL{filter}
         ORDER BY p.created_at, p.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs(&params).as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            PanelRecord {
                template_slug: row.get(1)?,
                markdown: row.get(2)?,
            },
        ))
    })?;
    let mut by_meeting: HashMap<String, Vec<PanelRecord>> = HashMap::new();
    for row in rows {
        let (id, panel) = row?;
        by_meeting.entry(id).or_default().push(panel);
    }
    Ok(by_meeting)
}

fn decode_record(
    id: String,
    title: String,
    created_at: Option<String>,
    people_json: Option<String>,
    calendar_json: Option<String>,
) -> Result<MeetingRecord> {
    let created_at = created_at.with_context(|| format!("meeting {id} has no created_at"))?;
    let people = people_json
        .map(|json| serde_json::from_str::<DocumentPeople>(&json))
        .transpose()
        .with_context(|| format!("meeting {id} has an unreadable people list"))?;
    let calendar = calendar_json
        .map(|json| serde_json::from_str::<CalendarInvite>(&json))
        .transpose()
        .with_context(|| format!("meeting {id} has an unreadable calendar event"))?;
    Ok(MeetingRecord {
        created_at: parse_timestamp(&created_at)
            .with_context(|| format!("meeting {id} has an unreadable created_at"))?,
        id,
        title,
        people,
        calendar,
        typed_note_chars: 0,
        panels: vec![],
    })
}

/// Call `each` once per meeting in `range` that has a transcript, with its
/// meeting id and utterances in start order. Streaming one meeting at a
/// time keeps memory flat across the whole corpus.
pub fn for_each_transcript(
    conn: &Connection,
    range: Option<&DateRange>,
    mut each: impl FnMut(&str, &[Utterance]),
) -> Result<()> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT u.document_id, u.id, u.start_timestamp, u.source, u.speaker_name,
                length(u.text), rtrim(u.text) LIKE '%?'
         FROM transcript_utterances u
         JOIN documents d ON d.id = u.document_id
         WHERE d.deleted_at IS NULL{filter}
         ORDER BY u.document_id, u.start_timestamp, u.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_refs(&params).as_slice())?;

    let mut current: Option<String> = None;
    let mut utterances = Vec::new();
    while let Some(row) = rows.next()? {
        let document_id: String = row.get(0)?;
        if current.as_deref() != Some(document_id.as_str()) {
            if let Some(done) = current.replace(document_id) {
                each(&done, &utterances);
                utterances.clear();
            }
        }
        utterances.push(decode_utterance(row)?);
    }
    if let Some(done) = current {
        each(&done, &utterances);
    }
    Ok(())
}

/// Call `each` with every utterance's text in `range`: its meeting id, the
/// meeting's creation time, whether it is the user's microphone, and the
/// text. Only the words statistics read text, so they stream it apart from
/// the rows.
pub fn for_each_utterance_text(
    conn: &Connection,
    range: Option<&DateRange>,
    mut each: impl FnMut(&str, DateTime<Utc>, bool, &str),
) -> Result<()> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT u.document_id, d.created_at, u.source = 'microphone', u.text
         FROM transcript_utterances u
         JOIN documents d ON d.id = u.document_id
         WHERE d.deleted_at IS NULL AND u.text IS NOT NULL{filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_refs(&params).as_slice())?;
    let mut meeting = String::new();
    let mut created_at = DateTime::<Utc>::MIN_UTC;
    while let Some(row) = rows.next()? {
        let id: &str = row.get_ref(0)?.as_str()?;
        if id != meeting {
            let created: String = row.get(1)?;
            created_at = parse_timestamp(&created)
                .with_context(|| format!("meeting {id} has an unreadable created_at"))?;
            meeting = id.to_string();
        }
        let mine: Option<bool> = row.get(2)?;
        each(
            &meeting,
            created_at,
            mine.unwrap_or(false),
            row.get_ref(3)?.as_str()?,
        );
    }
    Ok(())
}

/// Each meeting's transcript embedding: the mean of its transcript
/// windows' vectors, for meetings in `range` that `grans embed` has
/// reached. Notes and AI-summary chunks are left out, so a meeting is
/// represented by what was said.
pub fn meeting_vectors(
    conn: &Connection,
    range: Option<&DateRange>,
) -> Result<HashMap<String, Vec<f32>>> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT c.document_id, e.vector
         FROM chunks c
         JOIN embeddings e ON e.chunk_id = c.id
         JOIN documents d ON d.id = c.document_id
         WHERE c.source_type = 'transcript_window' AND d.deleted_at IS NULL{filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_refs(&params).as_slice())?;
    let mut sums: HashMap<String, (Vec<f32>, u32)> = HashMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let vector = blob_to_vector(row.get_ref(1)?.as_blob()?);
        let (sum, n) = sums
            .entry(id)
            .or_insert_with(|| (vec![0.0; vector.len()], 0));
        if sum.len() != vector.len() {
            anyhow::bail!(
                "embeddings of different lengths in one database; rebuild them with `grans embed clear` and `grans embed`"
            );
        }
        for (s, v) in sum.iter_mut().zip(&vector) {
            *s += v;
        }
        *n += 1;
    }
    Ok(sums
        .into_iter()
        .map(|(id, (sum, n))| (id, sum.into_iter().map(|s| s / n as f32).collect()))
        .collect())
}

fn decode_utterance(row: &rusqlite::Row) -> Result<Utterance> {
    let id: String = row.get(1)?;
    let start: Option<String> = row.get(2)?;
    let start = start.with_context(|| format!("utterance {id} has no start time"))?;
    Ok(Utterance {
        start: parse_timestamp(&start)
            .with_context(|| format!("utterance {id} has an unreadable start time"))?,
        voice: Voice::from_columns(row.get::<_, Option<String>>(3)?.as_deref(), row.get(4)?)
            .with_context(|| format!("utterance {id}"))?,
        chars: row.get::<_, Option<u32>>(5)?.unwrap_or(0),
        question: row.get::<_, Option<bool>>(6)?.unwrap_or(false),
    })
}

fn parse_timestamp(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc))
}

/// The `created_at` bounds as a SQL fragment on `d`, matching how `list`
/// filters meetings by date.
fn range_filter(range: Option<&DateRange>) -> (String, Vec<String>) {
    let mut filter = String::new();
    let mut params = Vec::new();
    if let Some(start) = range.and_then(|r| r.start) {
        filter.push_str(" AND d.created_at >= ?");
        params.push(start.to_rfc3339());
    }
    if let Some(end) = range.and_then(|r| r.end) {
        filter.push_str(" AND d.created_at < ?");
        params.push(end.to_rfc3339());
    }
    (filter, params)
}

fn params_refs(params: &[String]) -> Vec<&dyn ToSql> {
    params.iter().map(|p| p as &dyn ToSql).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn collect_transcripts(
        conn: &Connection,
        range: Option<&DateRange>,
    ) -> Vec<(String, Vec<Utterance>)> {
        let mut seen = Vec::new();
        for_each_transcript(conn, range, |id, utts| {
            seen.push((id.to_string(), utts.to_vec()))
        })
        .unwrap();
        seen
    }

    fn two_meetings() -> serde_json::Value {
        json!({
            "documents": {
                "doc-a": {"id": "doc-a", "title": "Planning", "created_at": "2026-03-02T16:00:00.000Z",
                          "google_calendar_event": {"recurringEventId": "series-1"}},
                "doc-b": {"id": "doc-b", "title": "Retro", "created_at": "2026-03-09T16:00:00.000Z"}
            },
            "transcripts": {
                "doc-a": [
                    {"id": "a2", "document_id": "doc-a", "start_timestamp": "2026-03-02T16:01:00.000Z",
                     "text": "Sounds good. ", "source": "system", "speaker_name": "Ana"},
                    {"id": "a1", "document_id": "doc-a", "start_timestamp": "2026-03-02T16:00:30.000Z",
                     "text": "Ready?", "source": "microphone"}
                ],
                "doc-b": [
                    {"id": "b1", "document_id": "doc-b", "start_timestamp": "2026-03-09T16:00:10.000Z",
                     "text": "Hi", "source": "system"}
                ]
            }
        })
    }

    #[test]
    fn notes_of_only_blank_lines_are_not_typed_notes() {
        let conn = build_test_db(&json!({
            "documents": {
                "blank": {"id": "blank", "title": "T", "created_at": "2026-03-02T16:00:00Z",
                          "notes_plain": "\n \r\n\t"},
                "typed": {"id": "typed", "title": "T", "created_at": "2026-03-03T16:00:00Z",
                          "notes_plain": "\n Ship it \n"}
            }
        }));

        let chars: Vec<_> = meeting_records(&conn, None)
            .unwrap()
            .into_iter()
            .map(|r| r.typed_note_chars)
            .collect();
        assert_eq!(chars, vec![0, 7]);
    }

    #[test]
    fn records_decode_the_calendar_event() {
        let conn = build_test_db(&two_meetings());

        let records = meeting_records(&conn, None).unwrap();

        let series: Vec<_> = records
            .iter()
            .map(|r| {
                r.calendar
                    .as_ref()
                    .and_then(|c| c.recurring_event_id.clone())
            })
            .collect();
        assert_eq!(series, vec![Some("series-1".to_string()), None]);
    }

    #[test]
    fn records_honor_the_date_range() {
        let conn = build_test_db(&two_meetings());
        let range = DateRange {
            start: Some("2026-03-05T00:00:00Z".parse().unwrap()),
            end: None,
        };

        let ids: Vec<_> = meeting_records(&conn, Some(&range))
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["doc-b"]);
    }

    #[test]
    fn transcripts_arrive_one_meeting_at_a_time_in_start_order() {
        let conn = build_test_db(&two_meetings());

        let seen = collect_transcripts(&conn, None);

        assert_eq!(
            seen,
            vec![
                (
                    "doc-a".to_string(),
                    vec![
                        Utterance {
                            start: "2026-03-02T16:00:30Z".parse().unwrap(),
                            voice: Voice::Me,
                            chars: 6,
                            question: true,
                        },
                        Utterance {
                            start: "2026-03-02T16:01:00Z".parse().unwrap(),
                            voice: Voice::Named("Ana".into()),
                            chars: 13,
                            question: false,
                        },
                    ]
                ),
                (
                    "doc-b".to_string(),
                    vec![Utterance {
                        start: "2026-03-09T16:00:10Z".parse().unwrap(),
                        voice: Voice::Others,
                        chars: 2,
                        question: false,
                    }]
                ),
            ]
        );
    }

    #[test]
    fn text_arrives_with_its_meeting_and_channel() {
        let conn = build_test_db(&two_meetings());

        let mut seen = Vec::new();
        for_each_utterance_text(&conn, None, |id, _, mine, text| {
            seen.push((id.to_string(), mine, text.to_string()))
        })
        .unwrap();
        seen.sort();

        assert_eq!(
            seen,
            vec![
                ("doc-a".to_string(), false, "Sounds good. ".to_string()),
                ("doc-a".to_string(), true, "Ready?".to_string()),
                ("doc-b".to_string(), false, "Hi".to_string()),
            ]
        );
    }

    #[test]
    fn a_meetings_vector_is_the_mean_of_its_transcript_windows() {
        let conn = build_test_db(&two_meetings());
        let chunk = |source_type: &str, vector: [f32; 2]| {
            conn.execute(
                "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
                 VALUES (?1, ?2, 'doc-a', 'h', 't', '2026-03-02T16:00:00Z')",
                rusqlite::params![source_type, format!("{source_type}-{}", vector[0])],
            )
            .unwrap();
            let blob: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                rusqlite::params![conn.last_insert_rowid(), blob],
            )
            .unwrap();
        };
        chunk("transcript_window", [1.0, 0.0]);
        chunk("transcript_window", [3.0, 2.0]);
        chunk("panel_section", [100.0, 100.0]);

        let vectors = meeting_vectors(&conn, None).unwrap();

        assert_eq!(
            vectors,
            HashMap::from([("doc-a".to_string(), vec![2.0, 1.0])])
        );
    }

    #[test]
    fn utterance_without_start_time_is_an_error() {
        let conn = build_test_db(&json!({
            "documents": {"doc-a": {"id": "doc-a", "title": "T", "created_at": "2026-03-02T16:00:00Z"}},
            "transcripts": {"doc-a": [{"id": "a1", "document_id": "doc-a", "text": "Hi", "source": "system"}]}
        }));

        let result = for_each_transcript(&conn, None, |_, _| {});

        assert!(result.is_err());
    }
}
