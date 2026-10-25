//! The two reads statistics are built on: each meeting's document fields,
//! and its utterances reduced to [`Utterance`]s. JSON columns and timestamps
//! are decoded here, so `stats` works with typed values only.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use rusqlite::types::ToSql;

use crate::models::DocumentPeople;
use crate::query::dates::DateRange;
use crate::stats::calendar::CalendarInvite;
use crate::stats::{MeetingRecord, Utterance, Voice};

/// Every meeting created in `range`, oldest first. Deleted meetings are
/// left out.
pub fn meeting_records(conn: &Connection, range: Option<&DateRange>) -> Result<Vec<MeetingRecord>> {
    let (filter, params) = range_filter(range);
    let sql = format!(
        "SELECT id, title, created_at, people_json, google_calendar_event_json
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
        ))
    })?;

    rows.map(|row| {
        let (id, title, created_at, people_json, calendar_json) = row?;
        decode_record(id, title, created_at, people_json, calendar_json)
    })
    .collect()
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
    fn utterance_without_start_time_is_an_error() {
        let conn = build_test_db(&json!({
            "documents": {"doc-a": {"id": "doc-a", "title": "T", "created_at": "2026-03-02T16:00:00Z"}},
            "transcripts": {"doc-a": [{"id": "a1", "document_id": "doc-a", "text": "Hi", "source": "system"}]}
        }));

        let result = for_each_transcript(&conn, None, |_, _| {});

        assert!(result.is_err());
    }
}
