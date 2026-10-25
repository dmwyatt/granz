//! The headline numbers `grans stats` prints with no subcommand.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{MeetingRow, TranscriptFacts};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Overview {
    pub meetings: usize,
    pub with_transcript: usize,
    pub first_meeting: Option<DateTime<Utc>>,
    pub last_meeting: Option<DateTime<Utc>>,
    /// First spoken word to last, summed over meetings.
    pub spoken_hours: f64,
    /// The user's share of transcribed characters, 0 to 1. `None` when
    /// nothing was transcribed.
    pub my_share: Option<f64>,
    /// When the earliest meeting with a detected speaker name was created:
    /// statistics about named speakers cover only meetings since then.
    pub speaker_names_since: Option<DateTime<Utc>>,
    pub meetings_with_speaker_names: usize,
}

pub fn overview(rows: &[MeetingRow]) -> Overview {
    let transcripts: Vec<&TranscriptFacts> =
        rows.iter().filter_map(|r| r.transcript.as_ref()).collect();
    let named: Vec<&MeetingRow> = rows
        .iter()
        .filter(|r| {
            r.transcript
                .as_ref()
                .is_some_and(|t| t.named_speakers() > 0)
        })
        .collect();
    let mine: u64 = transcripts.iter().map(|t| t.my_chars()).sum();
    let others: u64 = transcripts.iter().map(|t| t.others_chars()).sum();

    Overview {
        meetings: rows.len(),
        with_transcript: transcripts.len(),
        first_meeting: rows.iter().map(|r| r.created_at).min(),
        last_meeting: rows.iter().map(|r| r.created_at).max(),
        spoken_hours: transcripts
            .iter()
            .map(|t| t.span().num_seconds() as f64 / 3600.0)
            .sum(),
        my_share: share(mine, others),
        speaker_names_since: named.iter().map(|r| r.created_at).min(),
        meetings_with_speaker_names: named.len(),
    }
}

/// `part` as a fraction of `part + rest`, or `None` when both are zero.
pub fn share(part: u64, rest: u64) -> Option<f64> {
    let total = part + rest;
    (total > 0).then(|| part as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{MeetingRecord, Utterance, Voice, summarize};

    fn meeting(created: &str, utterances: &[(&str, Voice, u32)]) -> MeetingRow {
        let utterances: Vec<Utterance> = utterances
            .iter()
            .map(|(start, voice, chars)| Utterance {
                start: start.parse().unwrap(),
                voice: voice.clone(),
                chars: *chars,
                question: false,
            })
            .collect();
        MeetingRow::new(
            MeetingRecord {
                id: created.to_string(),
                title: "Sync".to_string(),
                created_at: created.parse().unwrap(),
                ..Default::default()
            },
            summarize(&utterances),
        )
    }

    #[test]
    fn my_share_is_pooled_over_characters_not_averaged_per_meeting() {
        let rows = [
            meeting(
                "2026-03-02T16:00:00Z",
                &[
                    ("2026-03-02T16:00:00Z", Voice::Me, 10),
                    ("2026-03-02T16:01:00Z", Voice::Others, 10),
                ],
            ),
            meeting(
                "2026-03-03T16:00:00Z",
                &[("2026-03-03T16:00:00Z", Voice::Others, 80)],
            ),
        ];

        assert_eq!(overview(&rows).my_share, Some(0.1));
    }

    #[test]
    fn speaker_names_since_is_the_first_meeting_with_a_named_voice() {
        let rows = [
            meeting(
                "2026-07-01T16:00:00Z",
                &[("2026-07-01T16:00:00Z", Voice::Others, 5)],
            ),
            meeting(
                "2026-07-22T16:00:00Z",
                &[("2026-07-22T16:00:00Z", Voice::Named("Ana".into()), 5)],
            ),
            meeting("2026-07-23T16:00:00Z", &[]),
        ];

        let o = overview(&rows);
        assert_eq!(
            (o.speaker_names_since, o.meetings_with_speaker_names),
            (Some("2026-07-22T16:00:00Z".parse().unwrap()), 1)
        );
    }

    #[test]
    fn spoken_hours_sum_first_to_last_word() {
        let rows = [
            meeting(
                "2026-03-02T16:00:00Z",
                &[
                    ("2026-03-02T16:00:00Z", Voice::Me, 1),
                    ("2026-03-02T16:45:00Z", Voice::Me, 1),
                ],
            ),
            meeting(
                "2026-03-03T16:00:00Z",
                &[
                    ("2026-03-03T16:00:00Z", Voice::Me, 1),
                    ("2026-03-03T16:15:00Z", Voice::Me, 1),
                ],
            ),
        ];

        assert_eq!(overview(&rows).spoken_hours, 1.0);
    }

    #[test]
    fn meeting_without_transcript_counts_as_a_meeting_only() {
        let rows = [meeting("2026-03-02T16:00:00Z", &[])];

        let o = overview(&rows);
        assert_eq!((o.meetings, o.with_transcript, o.my_share), (1, 0, None));
    }
}
