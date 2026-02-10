use anyhow::Result;
use rusqlite::Connection;

use crate::models::{Calendar, CalendarEvent, EventDateTime};
use crate::query::dates::DateRange;

struct CalendarEventRow {
    id: Option<String>,
    summary: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    calendar_id: Option<String>,
    attendees_json: Option<String>,
    conference_data_json: Option<String>,
    description: Option<String>,
    extra_json: Option<String>,
}

fn row_to_calendar_event(row: CalendarEventRow) -> CalendarEvent {
    let attendees = row
        .attendees_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let conference_data = row
        .conference_data_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let extra = row
        .extra_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    CalendarEvent {
        id: row.id,
        summary: row.summary,
        description: row.description,
        start: row.start_time.map(|dt| EventDateTime {
            date_time: Some(dt),
            time_zone: None,
            extra: Default::default(),
        }),
        end: row.end_time.map(|dt| EventDateTime {
            date_time: Some(dt),
            time_zone: None,
            extra: Default::default(),
        }),
        calendar_id: row.calendar_id,
        attendees,
        creator: None,
        organizer: None,
        conference_data,
        recurring_event_id: None,
        ical_uid: None,
        status: None,
        html_link: None,
        extra,
    }
}

pub fn list_calendars(conn: &Connection) -> Result<Vec<Calendar>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, \"primary\", access_role, summary, background_color FROM calendars",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Calendar {
            id: row.get(0)?,
            provider: row.get(1)?,
            primary: row.get::<_, Option<bool>>(2)?,
            access_role: row.get(3)?,
            summary: row.get(4)?,
            background_color: row.get(5)?,
            extra: Default::default(),
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn list_events(
    conn: &Connection,
    calendar: Option<&str>,
    date_range: Option<&DateRange>,
) -> Result<Vec<CalendarEvent>> {
    let mut sql = String::from("SELECT id, summary, start_time, end_time, calendar_id, attendees_json, conference_data_json, description, extra_json FROM events WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(cal) = calendar {
        sql.push_str(" AND calendar_id LIKE ?");
        params.push(Box::new(format!("%{}%", cal)));
    }

    if let Some(range) = date_range {
        if let Some(start) = &range.start {
            sql.push_str(" AND start_time >= ?");
            params.push(Box::new(start.to_rfc3339()));
        }
        if let Some(end) = &range.end {
            sql.push_str(" AND start_time < ?");
            params.push(Box::new(end.to_rfc3339()));
        }
    }

    sql.push_str(" ORDER BY start_time DESC");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(CalendarEventRow {
            id: row.get(0)?,
            summary: row.get(1)?,
            start_time: row.get(2)?,
            end_time: row.get(3)?,
            calendar_id: row.get(4)?,
            attendees_json: row.get(5)?,
            conference_data_json: row.get(6)?,
            description: row.get(7)?,
            extra_json: row.get(8)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_calendar_event).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, calendars_state};

    #[test]
    fn test_list_calendars() {
        let conn = build_test_db(&calendars_state());
        let cals = list_calendars(&conn).unwrap();
        assert_eq!(cals.len(), 2);
    }

    #[test]
    fn test_list_events_no_filter() {
        let conn = build_test_db(&calendars_state());
        let events = list_events(&conn, None, None).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_list_events_filter_by_calendar() {
        let conn = build_test_db(&calendars_state());
        let events = list_events(&conn, Some("cal-1"), None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary.as_deref(), Some("Morning Standup"));
    }

    #[test]
    fn test_list_events_filter_by_date() {
        let conn = build_test_db(&calendars_state());
        use chrono::{TimeZone, Utc};
        let range = DateRange {
            start: Some(Utc.with_ymd_and_hms(2026, 1, 21, 0, 0, 0).unwrap()),
            end: Some(Utc.with_ymd_and_hms(2026, 1, 22, 0, 0, 0).unwrap()),
        };
        let events = list_events(&conn, None, Some(&range)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary.as_deref(), Some("Afternoon Meeting"));
    }

    #[test]
    fn test_row_to_calendar_event_all_fields() {
        let row = CalendarEventRow {
            id: Some("e-1".to_string()),
            summary: Some("Team Meeting".to_string()),
            start_time: Some("2026-01-20T10:00:00Z".to_string()),
            end_time: Some("2026-01-20T11:00:00Z".to_string()),
            calendar_id: Some("cal-1".to_string()),
            attendees_json: Some(r#"[{"email":"alice@example.com"}]"#.to_string()),
            conference_data_json: Some(r#"{"entryPointUri":"https://meet.google.com/abc"}"#.to_string()),
            description: Some("Discuss project".to_string()),
            extra_json: Some(r#"{"custom":"value"}"#.to_string()),
        };
        let event = row_to_calendar_event(row);
        assert_eq!(event.id.as_deref(), Some("e-1"));
        assert_eq!(event.summary.as_deref(), Some("Team Meeting"));
        assert_eq!(event.description.as_deref(), Some("Discuss project"));
        assert_eq!(event.calendar_id.as_deref(), Some("cal-1"));

        // Start/end wrapped into EventDateTime
        let start = event.start.unwrap();
        assert_eq!(start.date_time.as_deref(), Some("2026-01-20T10:00:00Z"));
        assert!(start.time_zone.is_none());
        let end = event.end.unwrap();
        assert_eq!(end.date_time.as_deref(), Some("2026-01-20T11:00:00Z"));

        // JSON deserialized
        assert!(event.attendees.is_some());
        assert_eq!(event.attendees.unwrap().len(), 1);
        assert!(event.conference_data.is_some());
        assert_eq!(event.extra["custom"], "value");

        // Fields not in DB are None
        assert!(event.creator.is_none());
        assert!(event.organizer.is_none());
        assert!(event.recurring_event_id.is_none());
    }

    #[test]
    fn test_row_to_calendar_event_none_times() {
        let row = CalendarEventRow {
            id: Some("e-2".to_string()),
            summary: None,
            start_time: None,
            end_time: None,
            calendar_id: None,
            attendees_json: None,
            conference_data_json: None,
            description: None,
            extra_json: None,
        };
        let event = row_to_calendar_event(row);
        assert!(event.start.is_none());
        assert!(event.end.is_none());
        assert!(event.attendees.is_none());
        assert!(event.conference_data.is_none());
        assert!(event.extra.is_empty());
    }
}
