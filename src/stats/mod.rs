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
#[cfg(test)]
mod fixtures;
mod invitees;
mod mentions;
pub mod notes;
pub mod overview;
pub mod page;
pub mod people;
pub mod series;
pub mod speakers;
pub mod talk;
pub mod time;
pub mod topics;
mod transcript;
pub mod words;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::models::DocumentPeople;
use calendar::{CalendarInvite, Rsvp};
use notes::{PanelRecord, Summary};

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
    /// The user's email, lowercased: the calendar's own attendee entry, or
    /// else the creator of Granola's note.
    pub my_email: Option<String>,
    /// Everyone invited other than the user, including those who declined.
    /// An invitation says nothing about who actually came.
    pub invitees: Vec<Invitee>,
    /// The user's name as Granola's note gives its creator.
    pub my_name: Option<String>,
    /// `None` for a meeting with no transcript.
    pub transcript: Option<TranscriptFacts>,
    /// Characters of notes the user typed during the meeting.
    pub typed_note_chars: u32,
    /// Granola's AI summaries of the meeting.
    pub summaries: Vec<Summary>,
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
    pub typed_note_chars: u32,
    pub panels: Vec<PanelRecord>,
}

impl MeetingRow {
    pub fn new(record: MeetingRecord, transcript: Option<TranscriptFacts>) -> Self {
        let calendar = record.calendar.as_ref();
        let organizer = calendar.and_then(CalendarInvite::organizer_email);
        let my_email = calendar
            .and_then(CalendarInvite::my_email)
            .or_else(|| creator_email(record.people.as_ref()));
        MeetingRow {
            invitees: invitees::merge(calendar, record.people.as_ref()),
            series: calendar.and_then(|c| c.recurring_event_id.clone()),
            scheduled: calendar.and_then(CalendarInvite::schedule),
            // Google marks the organizer as `self` only sometimes; an
            // organizer with the user's own address is the user too.
            organized_by_me: calendar.is_some_and(CalendarInvite::organized_by_me)
                || (organizer.is_some() && organizer == my_email),
            organizer,
            my_response: calendar.and_then(CalendarInvite::my_response),
            my_email,
            my_name: creator_name(record.people.as_ref()),
            typed_note_chars: record.typed_note_chars,
            summaries: record.panels.iter().map(Summary::new).collect(),
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

fn creator_name(people: Option<&DocumentPeople>) -> Option<String> {
    people?.creator.as_ref()?.name.clone()
}

fn creator_email(people: Option<&DocumentPeople>) -> Option<String> {
    Some(people?.creator.as_ref()?.email.as_ref()?.to_lowercase())
}

/// The lower median, so the result is always one of the values.
pub fn median<T: Ord>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut values: Vec<T> = values.into_iter().collect();
    values.sort();
    let middle = values.len().checked_sub(1)? / 2;
    Some(values.swap_remove(middle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn my_email_falls_back_to_the_note_creator() {
        let people: DocumentPeople =
            serde_json::from_value(json!({"creator": {"email": "Me@Example.com"}})).unwrap();
        let record = MeetingRecord {
            people: Some(people),
            calendar: Some(CalendarInvite::default()),
            ..Default::default()
        };

        assert_eq!(
            MeetingRow::new(record, None).my_email.as_deref(),
            Some("me@example.com")
        );
    }

    #[test]
    fn an_organizer_with_my_address_is_me_without_the_self_flag() {
        let calendar: CalendarInvite = serde_json::from_value(json!({
            "organizer": {"email": "me@example.com"},
            "attendees": [{"email": "me@example.com", "self": true}]
        }))
        .unwrap();
        let record = MeetingRecord {
            calendar: Some(calendar),
            ..Default::default()
        };

        assert!(MeetingRow::new(record, None).organized_by_me);
    }

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
