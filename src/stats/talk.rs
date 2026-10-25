//! Conversation shape, from the two audio channels: how much the user
//! talks, how often the conversation passes back and forth, when in a
//! meeting the user speaks up, and how many questions each side asks.
//!
//! Everything here uses the channels, not speaker names, so it covers
//! every meeting with a transcript. Talk is measured in characters.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use serde::Serialize;

use super::overview::share;
use super::time::{SIZE_LABELS, size_bucket};
use super::transcript::VoiceRow;
use super::{MeetingRow, TranscriptFacts, Voice, median};

/// The user's share of the talk in a group of meetings, pooled over their
/// characters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShareGroup {
    pub label: String,
    pub meetings: usize,
    pub my_share: Option<f64>,
    /// Median handoffs between the user and everyone else per hour of
    /// talk. High is conversation; low is someone presenting.
    pub handoffs_per_hour: Option<f64>,
    /// Share of utterances ending in a question mark, per side.
    pub my_question_rate: Option<f64>,
    pub others_question_rate: Option<f64>,
}

/// A meeting named in a list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeetingRef {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TalkReport {
    pub overall: ShareGroup,
    /// By invitees who did not decline.
    pub by_size: Vec<ShareGroup>,
    pub by_month: Vec<ShareGroup>,
    /// The user's share in each tenth of a meeting's talk, pooled over
    /// meetings with a single session.
    pub by_tenth: Vec<Option<f64>>,
    /// Others talked and the user never did.
    pub silent: Vec<MeetingRef>,
    /// Only the user's microphone was heard: often an in-person meeting,
    /// where everyone is on the one microphone.
    pub microphone_only: Vec<MeetingRef>,
}

/// One meeting's numbers, before grouping.
struct Talk<'a> {
    row: &'a MeetingRow,
    facts: &'a TranscriptFacts,
}

impl Talk<'_> {
    fn sum(&self, keep: impl Fn(&Voice) -> bool, field: impl Fn(&VoiceRow) -> u64) -> u64 {
        self.facts
            .voices
            .iter()
            .filter(|v| keep(&v.voice))
            .map(field)
            .sum()
    }

    fn handoffs_per_hour(&self) -> Option<f64> {
        let hours = self.facts.span().num_seconds() as f64 / 3600.0;
        (hours > 0.0).then(|| f64::from(self.facts.handoffs) / hours)
    }
}

fn is_me(v: &Voice) -> bool {
    *v == Voice::Me
}

fn group(label: String, talks: &[&Talk]) -> ShareGroup {
    let total = |keep: fn(&Voice) -> bool, field: fn(&VoiceRow) -> u64| -> u64 {
        talks.iter().map(|t| t.sum(keep, field)).sum()
    };
    let chars = |v: &VoiceRow| v.chars;
    let utterances = |v: &VoiceRow| u64::from(v.utterances);
    let questions = |v: &VoiceRow| u64::from(v.questions);
    let rate = |q: u64, n: u64| (n > 0).then(|| q as f64 / n as f64);

    ShareGroup {
        label,
        meetings: talks.len(),
        my_share: share(total(is_me, chars), total(Voice::is_others, chars)),
        handoffs_per_hour: median(
            talks
                .iter()
                .filter_map(|t| t.handoffs_per_hour())
                .map(|h| (h * 100.0).round() as i64),
        )
        .map(|h| h as f64 / 100.0),
        my_question_rate: rate(total(is_me, questions), total(is_me, utterances)),
        others_question_rate: rate(
            total(Voice::is_others, questions),
            total(Voice::is_others, utterances),
        ),
    }
}

fn reference(row: &MeetingRow) -> MeetingRef {
    MeetingRef {
        id: row.id.clone(),
        title: row.title.clone(),
        created_at: row.created_at,
    }
}

pub fn report(rows: &[MeetingRow], tz: &FixedOffset) -> TalkReport {
    let talks: Vec<Talk> = rows
        .iter()
        .filter_map(|row| {
            Some(Talk {
                row,
                facts: row.transcript.as_ref()?,
            })
        })
        .collect();
    let all: Vec<&Talk> = talks.iter().collect();

    let mut sizes: BTreeMap<usize, Vec<&Talk>> = BTreeMap::new();
    let mut months: BTreeMap<NaiveDate, Vec<&Talk>> = BTreeMap::new();
    for t in &talks {
        sizes
            .entry(size_bucket(t.row.invited()))
            .or_default()
            .push(t);
        let local = t.row.created_at.with_timezone(tz).date_naive();
        let month = local.with_day(1).expect("every month has a first day");
        months.entry(month).or_default().push(t);
    }

    let mine = |t: &&Talk| t.sum(is_me, |v| v.chars);
    let theirs = |t: &&Talk| t.sum(Voice::is_others, |v| v.chars);

    TalkReport {
        overall: group("all".to_string(), &all),
        by_size: sizes
            .into_iter()
            .map(|(bucket, ts)| group(SIZE_LABELS[bucket].to_string(), &ts))
            .collect(),
        by_month: months
            .into_iter()
            .map(|(month, ts)| group(month.format("%Y-%m").to_string(), &ts))
            .collect(),
        by_tenth: by_tenth(&all),
        silent: all
            .iter()
            .filter(|t| mine(t) == 0 && theirs(t) > 0)
            .map(|t| reference(t.row))
            .collect(),
        microphone_only: all
            .iter()
            .filter(|t| mine(t) > 0 && theirs(t) == 0)
            .map(|t| reference(t.row))
            .collect(),
    }
}

/// Each single-session meeting's minutes are split into ten equal parts
/// and the user's and everyone else's characters summed per part.
fn by_tenth(talks: &[&Talk]) -> Vec<Option<f64>> {
    let mut mine = [0u64; 10];
    let mut theirs = [0u64; 10];
    for t in talks.iter().filter(|t| t.facts.sessions.len() == 1) {
        let minutes = t
            .facts
            .voices
            .iter()
            .map(|v| v.chars_by_minute.len())
            .max()
            .unwrap_or(0);
        for v in &t.facts.voices {
            let side = match v.voice {
                Voice::Me => &mut mine,
                Voice::Named(_) | Voice::Others => &mut theirs,
                Voice::Unknown => continue,
            };
            for (minute, chars) in v.chars_by_minute.iter().enumerate() {
                side[minute * 10 / minutes] += u64::from(*chars);
            }
        }
    }
    mine.iter().zip(theirs).map(|(m, o)| share(*m, o)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::{spoken, voice, with_voices};

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn binned(voice_kind: Voice, chars_by_minute: Vec<u32>) -> VoiceRow {
        VoiceRow {
            chars_by_minute: chars_by_minute.clone(),
            ..voice(
                voice_kind,
                chars_by_minute.iter().map(|c| u64::from(*c)).sum(),
            )
        }
    }

    #[test]
    fn a_meeting_where_only_others_spoke_is_silent() {
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![voice(Voice::Others, 500)],
        )];

        assert_eq!(report(&rows, &utc()).silent.len(), 1);
    }

    #[test]
    fn a_meeting_with_only_my_microphone_is_listed_apart() {
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![voice(Voice::Me, 500)],
        )];

        let r = report(&rows, &utc());
        assert_eq!((r.silent.len(), r.microphone_only.len()), (0, 1));
    }

    #[test]
    fn question_rate_is_per_side() {
        let mut me = voice(Voice::Me, 100);
        me.utterances = 10;
        me.questions = 1;
        let mut ana = voice(Voice::Named("Ana".into()), 100);
        ana.utterances = 4;
        ana.questions = 2;
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![me, ana],
        )];

        let o = report(&rows, &utc()).overall;
        assert_eq!(
            (o.my_question_rate, o.others_question_rate),
            (Some(0.1), Some(0.5))
        );
    }

    #[test]
    fn handoffs_are_counted_per_hour_of_talk() {
        let mut row = spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z");
        row.transcript.as_mut().unwrap().handoffs = 20;

        assert_eq!(report(&[row], &utc()).overall.handoffs_per_hour, Some(40.0));
    }

    #[test]
    fn tenths_split_each_meeting_by_its_own_length() {
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:09:00Z"),
            vec![
                binned(Voice::Me, vec![9, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                binned(Voice::Others, vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 9]),
            ],
        )];

        let tenths = report(&rows, &utc()).by_tenth;
        assert_eq!(
            (tenths[0], tenths[9], tenths[5]),
            (Some(0.9), Some(0.1), None)
        );
    }

    #[test]
    fn share_is_grouped_by_meeting_size() {
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![voice(Voice::Me, 1), voice(Voice::Others, 3)],
        )];

        let sizes: Vec<_> = report(&rows, &utc())
            .by_size
            .into_iter()
            .map(|g| (g.label, g.my_share))
            .collect();
        assert_eq!(sizes, vec![("none".to_string(), Some(0.25))]);
    }
}
