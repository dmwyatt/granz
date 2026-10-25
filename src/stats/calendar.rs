//! The parts of a meeting's Google Calendar event that statistics read.
//!
//! Granola stores the event as Google returned it. This decodes only the
//! fields statistics use, strictly: a field that is present but malformed is
//! an error rather than a silent gap in a chart.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use super::Schedule;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarInvite {
    #[serde(default)]
    pub start: Option<EventTime>,
    #[serde(default)]
    pub end: Option<EventTime>,
    #[serde(default)]
    pub recurring_event_id: Option<String>,
    #[serde(default)]
    pub organizer: Option<Organizer>,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
}

/// An event boundary. All-day events carry a `date` instead of a
/// `dateTime`, and have no slot to compare a meeting against.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTime {
    #[serde(default)]
    pub date_time: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Organizer {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(rename = "self", default)]
    pub is_self: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub response_status: Option<Rsvp>,
    #[serde(default)]
    pub organizer: bool,
    #[serde(rename = "self", default)]
    pub is_self: bool,
    /// A meeting room or other bookable resource, not a person.
    #[serde(default)]
    pub resource: bool,
}

/// An invitee's response, in Google Calendar's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Rsvp {
    Accepted,
    Declined,
    Tentative,
    /// Invited and never answered.
    NeedsAction,
}

impl CalendarInvite {
    pub fn schedule(&self) -> Option<Schedule> {
        let start = self.start.as_ref()?.date_time?;
        let end = self.end.as_ref()?.date_time?;
        Some(Schedule {
            start: start.with_timezone(&Utc),
            end: end.with_timezone(&Utc),
        })
    }

    pub fn organizer_email(&self) -> Option<String> {
        self.organizer
            .as_ref()?
            .email
            .as_deref()
            .map(str::to_lowercase)
    }

    pub fn organized_by_me(&self) -> bool {
        self.organizer.as_ref().is_some_and(|o| o.is_self)
    }

    pub fn my_response(&self) -> Option<Rsvp> {
        self.attendees
            .iter()
            .find(|a| a.is_self)
            .and_then(|a| a.response_status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(value: serde_json::Value) -> CalendarInvite {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn schedule_converts_the_event_offset_to_utc() {
        let invite = decode(json!({
            "start": {"dateTime": "2026-03-02T10:00:00-06:00", "timeZone": "America/Chicago"},
            "end": {"dateTime": "2026-03-02T10:30:00-06:00"}
        }));

        assert_eq!(
            invite.schedule(),
            Some(Schedule {
                start: "2026-03-02T16:00:00Z".parse().unwrap(),
                end: "2026-03-02T16:30:00Z".parse().unwrap(),
            })
        );
    }

    #[test]
    fn all_day_event_has_no_schedule() {
        let invite = decode(json!({
            "start": {"date": "2026-03-02"},
            "end": {"date": "2026-03-03"}
        }));

        assert_eq!(invite.schedule(), None);
    }

    #[test]
    fn my_response_comes_from_the_self_attendee() {
        let invite = decode(json!({
            "attendees": [
                {"email": "pat@example.com", "responseStatus": "accepted"},
                {"email": "me@example.com", "self": true, "responseStatus": "tentative"}
            ]
        }));

        assert_eq!(invite.my_response(), Some(Rsvp::Tentative));
    }

    #[test]
    fn unknown_response_status_is_an_error() {
        let result: Result<CalendarInvite, _> = serde_json::from_value(json!({
            "attendees": [{"email": "pat@example.com", "responseStatus": "maybe"}]
        }));

        assert!(result.is_err());
    }

    #[test]
    fn organizer_flag_reads_self() {
        let invite = decode(json!({"organizer": {"email": "Me@Example.com", "self": true}}));

        assert_eq!(
            (invite.organizer_email(), invite.organized_by_me()),
            (Some("me@example.com".to_string()), true)
        );
    }
}
