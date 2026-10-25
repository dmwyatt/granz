//! One meeting's transcript, reduced to who spoke how much and when.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Who an utterance belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "voice", content = "name", rename_all = "snake_case")]
pub enum Voice {
    /// The microphone channel: the user.
    Me,
    /// A system-channel utterance Granola attributed to a detected speaker,
    /// by display name.
    Named(String),
    /// A system-channel utterance with no detected speaker: everyone else,
    /// as one voice.
    Others,
    /// An utterance stored without a channel, which older syncs could write.
    Unknown,
}

impl Voice {
    /// Read the channel and detected speaker columns. Speaker names exist
    /// only on the system channel; the microphone is the user regardless.
    pub fn from_columns(source: Option<&str>, speaker_name: Option<String>) -> Result<Self> {
        Ok(match (source, speaker_name) {
            (Some("microphone"), _) => Voice::Me,
            (Some("system"), Some(name)) => Voice::Named(name),
            (Some("system"), None) => Voice::Others,
            (None, _) => Voice::Unknown,
            (Some(other), _) => bail!("unknown transcript channel {other:?}"),
        })
    }

    /// Whether this is someone other than the user, named or not.
    pub fn is_others(&self) -> bool {
        matches!(self, Voice::Named(_) | Voice::Others)
    }
}

/// What a statistic needs from one utterance: never its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub start: DateTime<Utc>,
    pub voice: Voice,
    pub chars: u32,
    /// Ends in a question mark.
    pub question: bool,
}

/// One voice's share of a meeting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceRow {
    #[serde(flatten)]
    pub voice: Voice,
    pub utterances: u32,
    pub chars: u64,
    pub questions: u32,
    pub first_word: DateTime<Utc>,
    pub last_word: DateTime<Utc>,
    /// Characters of utterances starting in each minute since the
    /// meeting's first word: entry 0 is the first minute.
    pub chars_by_minute: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptFacts {
    /// The earliest utterance start, on either channel.
    pub first_word: DateTime<Utc>,
    /// The latest utterance start. End timestamps are not trusted.
    pub last_word: DateTime<Utc>,
    /// Stretches of talk separated by more than [`SESSION_BREAK`] of
    /// silence, in order. Nearly every meeting has one; a note reopened
    /// days later to record another conversation has more.
    pub sessions: Vec<Session>,
    /// Times the conversation passed between the user and everyone else,
    /// in start order.
    pub handoffs: u32,
    /// The user first, then named speakers by name, then unnamed others.
    pub voices: Vec<VoiceRow>,
}

/// A silence longer than this ends one session of talk and starts another.
pub const SESSION_BREAK: Duration = Duration::minutes(30);

/// From the first utterance start of a stretch of talk to the last.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Session {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TranscriptFacts {
    /// Spoken time: the sessions' lengths, summed, so a meeting resumed
    /// days later does not count the days in between.
    pub fn span(&self) -> Duration {
        self.sessions
            .iter()
            .map(|s| s.end - s.start)
            .fold(Duration::zero(), |total, d| total + d)
    }

    pub fn my_chars(&self) -> u64 {
        self.chars_where(|v| *v == Voice::Me)
    }

    pub fn others_chars(&self) -> u64 {
        self.chars_where(Voice::is_others)
    }

    /// Distinct detected speakers. Zero for every meeting recorded before
    /// Granola detected speakers.
    pub fn named_speakers(&self) -> usize {
        self.voices
            .iter()
            .filter(|v| matches!(v.voice, Voice::Named(_)))
            .count()
    }

    fn chars_where(&self, keep: impl Fn(&Voice) -> bool) -> u64 {
        self.voices
            .iter()
            .filter(|v| keep(&v.voice))
            .map(|v| v.chars)
            .sum()
    }
}

/// Summarize one meeting's utterances, in any order. `None` when there are
/// none.
pub fn summarize(utterances: &[Utterance]) -> Option<TranscriptFacts> {
    let mut ordered: Vec<&Utterance> = utterances.iter().collect();
    ordered.sort_by_key(|u| u.start);
    let first_word = ordered.first()?.start;
    let last_word = ordered.last()?.start;

    let mut voices: BTreeMap<&Voice, VoiceRow> = BTreeMap::new();
    for u in &ordered {
        let row = voices.entry(&u.voice).or_insert_with(|| VoiceRow {
            voice: u.voice.clone(),
            utterances: 0,
            chars: 0,
            questions: 0,
            first_word: u.start,
            last_word: u.start,
            chars_by_minute: Vec::new(),
        });
        row.utterances += 1;
        row.chars += u64::from(u.chars);
        row.questions += u32::from(u.question);
        row.last_word = u.start;
        let minute = usize::try_from((u.start - first_word).num_minutes()).expect("in start order");
        if row.chars_by_minute.len() <= minute {
            row.chars_by_minute.resize(minute + 1, 0);
        }
        row.chars_by_minute[minute] += u.chars;
    }

    Some(TranscriptFacts {
        first_word,
        last_word,
        sessions: sessions(&ordered),
        handoffs: handoffs(&ordered),
        voices: voices.into_values().collect(),
    })
}

/// `ordered` must be non-empty and in start order.
fn sessions(ordered: &[&Utterance]) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for u in ordered {
        match sessions.last_mut() {
            Some(current) if u.start - current.end <= SESSION_BREAK => current.end = u.start,
            _ => sessions.push(Session {
                start: u.start,
                end: u.start,
            }),
        }
    }
    sessions
}

/// Channel changes between consecutive utterances whose channel is known.
fn handoffs(ordered: &[&Utterance]) -> u32 {
    let is_me: Vec<bool> = ordered
        .iter()
        .filter(|u| u.voice != Voice::Unknown)
        .map(|u| u.voice == Voice::Me)
        .collect();
    is_me.windows(2).filter(|w| w[0] != w[1]).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        format!("2026-03-02T16:{minute:02}:00Z").parse().unwrap()
    }

    fn said(minute: u32, voice: Voice, text: &str) -> Utterance {
        Utterance {
            start: at(minute),
            voice,
            chars: text.chars().count() as u32,
            question: text.ends_with('?'),
        }
    }

    fn named(name: &str) -> Voice {
        Voice::Named(name.to_string())
    }

    #[test]
    fn no_utterances_is_no_transcript() {
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn span_runs_from_first_to_last_start_whatever_the_input_order() {
        let facts = summarize(&[
            said(30, Voice::Others, "Thanks, all."),
            said(2, Voice::Me, "Shall we start?"),
        ])
        .unwrap();

        assert_eq!(facts.span(), Duration::minutes(28));
    }

    #[test]
    fn a_long_silence_splits_the_talk_into_sessions() {
        let facts = summarize(&[
            said(0, Voice::Me, "Hi"),
            said(20, Voice::Others, "Bye"),
            Utterance {
                start: "2026-03-04T09:00:00Z".parse().unwrap(),
                voice: Voice::Me,
                chars: 2,
                question: false,
            },
            Utterance {
                start: "2026-03-04T09:10:00Z".parse().unwrap(),
                voice: Voice::Others,
                chars: 3,
                question: false,
            },
        ])
        .unwrap();

        assert_eq!(facts.span(), Duration::minutes(30));
    }

    #[test]
    fn airtime_is_counted_in_characters_per_voice() {
        let facts = summarize(&[
            said(1, Voice::Me, "Hi"),
            said(2, named("Ana"), "Hello there"),
            said(3, Voice::Me, "Agenda?"),
            said(4, Voice::Others, "Yes"),
        ])
        .unwrap();

        let airtime: Vec<_> = facts
            .voices
            .iter()
            .map(|v| (v.voice.clone(), v.chars, v.questions))
            .collect();
        assert_eq!(
            airtime,
            vec![
                (Voice::Me, 9, 1),
                (named("Ana"), 11, 0),
                (Voice::Others, 3, 0)
            ]
        );
    }

    #[test]
    fn characters_are_binned_by_minute_since_the_first_word() {
        let facts = summarize(&[
            said(1, Voice::Others, "Hello"),
            said(3, Voice::Me, "Hi"),
            said(3, Voice::Me, "there"),
        ])
        .unwrap();

        let me = facts.voices.iter().find(|v| v.voice == Voice::Me).unwrap();
        assert_eq!(me.chars_by_minute, vec![0, 0, 7]);
    }

    #[test]
    fn named_and_unnamed_others_both_count_as_others() {
        let facts = summarize(&[
            said(1, Voice::Me, "Hi"),
            said(2, named("Ana"), "Hello"),
            said(3, Voice::Others, "Hey"),
        ])
        .unwrap();

        assert_eq!((facts.my_chars(), facts.others_chars()), (2, 8));
    }

    #[test]
    fn handoffs_count_changes_between_me_and_everyone_else() {
        let facts = summarize(&[
            said(1, Voice::Me, "a"),
            said(2, Voice::Me, "b"),
            said(3, Voice::Me, "c"),
            said(4, named("Ana"), "d"),
            said(5, named("Bo"), "e"),
            said(6, Voice::Me, "f"),
        ])
        .unwrap();

        assert_eq!(facts.handoffs, 2);
    }

    #[test]
    fn named_speakers_counts_distinct_names_only() {
        let facts = summarize(&[
            said(1, named("Ana"), "a"),
            said(2, named("Bo"), "b"),
            said(3, named("Ana"), "c"),
            said(4, Voice::Others, "d"),
        ])
        .unwrap();

        assert_eq!(facts.named_speakers(), 2);
    }

    #[test]
    fn microphone_is_me_even_with_a_speaker_name() {
        let voice = Voice::from_columns(Some("microphone"), Some("Ana".into())).unwrap();

        assert_eq!(voice, Voice::Me);
    }

    #[test]
    fn unrecognized_channel_is_an_error() {
        assert!(Voice::from_columns(Some("phone"), None).is_err());
    }
}
