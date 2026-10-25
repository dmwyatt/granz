//! Rows for statistics tests. Each builder sets exactly what its arguments
//! name; anything else a test needs it sets on the returned row.

use super::transcript::{Session, VoiceRow};
use super::{Invitee, MeetingRecord, MeetingRow, TranscriptFacts, Voice, calendar::Rsvp};

/// A meeting with one session of talk from `first_word` to `last_word`,
/// created at its first word, with no voices recorded.
pub fn spoken(first_word: &str, last_word: &str) -> MeetingRow {
    spoken_in(&[(first_word, last_word)])
}

/// A meeting with a session of talk per `(start, end)`, created at the
/// first start, with no voices recorded.
pub fn spoken_in(sessions: &[(&str, &str)]) -> MeetingRow {
    let sessions: Vec<Session> = sessions
        .iter()
        .map(|(start, end)| Session {
            start: start.parse().unwrap(),
            end: end.parse().unwrap(),
        })
        .collect();
    let mut row = untranscribed(sessions[0].start.to_rfc3339().as_str());
    row.transcript = Some(TranscriptFacts {
        first_word: sessions[0].start,
        last_word: sessions[sessions.len() - 1].end,
        sessions,
        handoffs: 0,
        voices: vec![],
        turns: vec![],
    });
    row
}

/// A meeting created at `created` with no transcript.
pub fn untranscribed(created: &str) -> MeetingRow {
    MeetingRow::new(
        MeetingRecord {
            id: created.to_string(),
            title: "Meeting".to_string(),
            created_at: created.parse().unwrap(),
            ..Default::default()
        },
        None,
    )
}

pub fn invitee(email: &str, response: Option<Rsvp>) -> Invitee {
    Invitee {
        email: email.to_string(),
        name: None,
        response,
        organizer: false,
    }
}

/// One voice's line, with `chars` of talk in one utterance and nothing
/// binned by minute. Its first and last word are placeholders.
pub fn voice(voice: Voice, chars: u64) -> VoiceRow {
    VoiceRow {
        voice,
        utterances: 1,
        chars,
        questions: 0,
        first_word: Default::default(),
        last_word: Default::default(),
        chars_by_minute: vec![],
    }
}

/// `row` with its transcript's voices replaced. The row must have a
/// transcript.
pub fn with_voices(mut row: MeetingRow, voices: Vec<VoiceRow>) -> MeetingRow {
    row.transcript.as_mut().expect("a transcript").voices = voices;
    row
}
