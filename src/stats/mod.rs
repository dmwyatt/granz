//! Meeting statistics, computed at query time from data grans already stores.
//!
//! Everything is built on one row per meeting, [`MeetingRow`], which carries
//! the meeting's calendar facts, its invitees, and a [`VoiceRow`] per speaker
//! summarizing the transcript. Each statistic is an aggregation over those
//! rows, so nothing here reads the database or stores a result.
//!
//! Two properties of the transcript data shape every statistic:
//!
//! - Utterance end timestamps are unreliable (many end minutes after the
//!   next utterance starts), so airtime is measured in characters, and only
//!   start timestamps are used, for order and gaps.
//! - The audio channel says only "you" (microphone) or "everyone else"
//!   (system). Granola's detected speaker names sit on top of the system
//!   channel, only for meetings recorded since it started detecting them, and
//!   are never backfilled. A statistic that spans that date has two eras.

pub mod calendar;
mod invitees;
pub mod overview;
pub mod page;
mod transcript;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::models::DocumentPeople;
use calendar::{CalendarInvite, Rsvp};

pub use invitees::Invitee;
pub use transcript::{TranscriptFacts, Utterance, Voice, summarize};

/// What grans knows about one meeting, before any aggregation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeetingRow {
    pub id: String,
    pub title: String,
    /// When the meeting's Granola document was created, which is when
    /// recording started for nearly every meeting. Date filters apply to it.
    pub created_at: DateTime<Utc>,
    /// The calendar's recurring event id, shared by every instance of a
    /// recurring meeting.
    pub series: Option<String>,
    pub scheduled: Option<Schedule>,
    /// The organizer's email, as the calendar gives it.
    pub organizer: Option<String>,
    pub organized_by_me: bool,
    /// The user's own response to the invitation.
    pub my_response: Option<Rsvp>,
    /// Everyone invited other than the user, including those who declined.
    /// An invitation says nothing about who actually came.
    pub invitees: Vec<Invitee>,
    /// `None` for a meeting with no transcript.
    pub transcript: Option<TranscriptFacts>,
}

/// The meeting's slot on the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Schedule {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// One meeting's stored document fields, decoded but not yet combined.
#[derive(Debug, Clone, Default)]
pub struct MeetingRecord {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub people: Option<DocumentPeople>,
    pub calendar: Option<CalendarInvite>,
}

impl MeetingRow {
    pub fn new(record: MeetingRecord, transcript: Option<TranscriptFacts>) -> Self {
        let calendar = record.calendar.as_ref();
        MeetingRow {
            invitees: invitees::merge(calendar, record.people.as_ref()),
            series: calendar.and_then(|c| c.recurring_event_id.clone()),
            scheduled: calendar.and_then(CalendarInvite::schedule),
            organizer: calendar.and_then(|c| c.organizer_email()),
            organized_by_me: calendar.is_some_and(CalendarInvite::organized_by_me),
            my_response: calendar.and_then(CalendarInvite::my_response),
            id: record.id,
            title: record.title,
            created_at: record.created_at,
            transcript,
        }
    }

    /// Invitees who did not decline: the meeting's size as the calendar
    /// sees it, not counting the user.
    pub fn invited(&self) -> usize {
        self.invitees
            .iter()
            .filter(|i| i.response != Some(Rsvp::Declined))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meeting_size_leaves_out_invitees_who_declined() {
        let calendar: CalendarInvite = serde_json::from_value(json!({"attendees": [
            {"email": "ana@example.com", "responseStatus": "accepted"},
            {"email": "bo@example.com", "responseStatus": "declined"},
            {"email": "cy@example.com", "responseStatus": "needsAction"}
        ]}))
        .unwrap();
        let record = MeetingRecord {
            calendar: Some(calendar),
            ..Default::default()
        };

        assert_eq!(MeetingRow::new(record, None).invited(), 2);
    }
}
