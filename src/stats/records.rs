//! The record book: the most of everything, and which meeting or day set
//! each record.

use chrono::{FixedOffset, NaiveDate, NaiveTime, Timelike};
use serde::Serialize;

use super::talk::MeetingRef;
use super::time::days;
use super::{MeetingRow, Voice};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub name: &'static str,
    pub value: f64,
    pub unit: &'static str,
    /// The meeting that set it, for records about one meeting.
    pub meeting: Option<MeetingRef>,
    /// The day that set it, for records about a day.
    pub day: Option<NaiveDate>,
    /// Who, for records about a speaker.
    pub who: Option<String>,
}

fn reference(row: &MeetingRow) -> MeetingRef {
    MeetingRef {
        id: row.id.clone(),
        title: row.title.clone(),
        created_at: row.created_at,
    }
}

fn hours(time: NaiveTime) -> f64 {
    f64::from(time.hour()) + f64::from(time.minute()) / 60.0
}

/// The meeting maximizing `value`, as a record. Earlier meetings win ties.
fn meeting_record(
    rows: &[MeetingRow],
    name: &'static str,
    unit: &'static str,
    value: impl Fn(&MeetingRow) -> Option<f64>,
) -> Option<Record> {
    let (row, best) = rows.iter().filter_map(|r| Some((r, value(r)?))).fold(
        None::<(&MeetingRow, f64)>,
        |best, (r, v)| match best {
            Some((_, b)) if b >= v => best,
            _ => Some((r, v)),
        },
    )?;
    Some(Record {
        name,
        value: best,
        unit,
        meeting: Some(reference(row)),
        day: None,
        who: None,
    })
}

/// The longest run by one person's voice that `keep` accepts, with who
/// said it. Unnamed talk from everyone else can join many people into one
/// run, so it never competes.
fn monologue(
    rows: &[MeetingRow],
    name: &'static str,
    keep: impl Fn(&Voice) -> bool,
) -> Option<Record> {
    let (row, voice, chars) = rows
        .iter()
        .flat_map(|r| {
            r.transcript
                .iter()
                .flat_map(|t| &t.voices)
                .filter(|v| matches!(v.voice, Voice::Me | Voice::Named(_)) && keep(&v.voice))
                .map(move |v| (r, &v.voice, v.longest_run))
        })
        .fold(
            None::<(&MeetingRow, &Voice, u64)>,
            |best, next| match best {
                Some((_, _, b)) if b >= next.2 => best,
                _ => Some(next),
            },
        )?;
    Some(Record {
        name,
        value: chars as f64,
        unit: "characters",
        meeting: Some(reference(row)),
        day: None,
        who: Some(match voice {
            Voice::Named(n) => n.clone(),
            _ => "You".to_string(),
        }),
    })
}

/// Back and forth needs some length to mean anything.
const MIN_MINUTES_FOR_RATE: i64 = 10;

pub fn report(rows: &[MeetingRow], tz: &FixedOffset) -> Vec<Record> {
    let days = days(rows, tz);
    let day_record = |name: &'static str, unit: &'static str, value: f64, day: NaiveDate| Record {
        name,
        value,
        unit,
        meeting: None,
        day: Some(day),
        who: None,
    };

    [
        meeting_record(rows, "Longest meeting", "minutes", |r| {
            Some(r.transcript.as_ref()?.span().num_seconds() as f64 / 60.0)
        }),
        monologue(rows, "Longest monologue", |_| true),
        monologue(rows, "Your longest monologue", |v| *v == Voice::Me)
            .map(|r| Record { who: None, ..r }),
        meeting_record(rows, "Biggest invite list", "invitees", |r| {
            Some(r.invited() as f64)
        }),
        meeting_record(rows, "Most speakers heard", "speakers", |r| {
            Some(r.transcript.as_ref()?.named_speakers() as f64).filter(|n| *n > 0.0)
        }),
        meeting_record(rows, "Most questions you asked", "questions", |r| {
            let t = r.transcript.as_ref()?;
            let mine = t.voices.iter().find(|v| v.voice == Voice::Me)?;
            Some(f64::from(mine.questions))
        }),
        meeting_record(rows, "Most back and forth", "handoffs per hour", |r| {
            let t = r.transcript.as_ref()?;
            let minutes = t.span().num_minutes();
            (minutes >= MIN_MINUTES_FOR_RATE).then(|| f64::from(t.handoffs) * 60.0 / minutes as f64)
        }),
        days.iter()
            .max_by(|a, b| a.hours.total_cmp(&b.hours).then(b.date.cmp(&a.date)))
            .map(|d| day_record("Busiest day", "hours", d.hours, d.date)),
        days.iter()
            .max_by(|a, b| a.meetings.cmp(&b.meetings).then(b.date.cmp(&a.date)))
            .map(|d| {
                day_record(
                    "Most meetings in a day",
                    "meetings",
                    d.meetings as f64,
                    d.date,
                )
            }),
        days.iter()
            .max_by(|a, b| {
                a.longest_streak
                    .cmp(&b.longest_streak)
                    .then(b.date.cmp(&a.date))
            })
            .map(|d| {
                day_record(
                    "Longest back-to-back run",
                    "meetings",
                    d.longest_streak as f64,
                    d.date,
                )
            }),
        days.iter()
            .min_by(|a, b| a.first_word.cmp(&b.first_word).then(a.date.cmp(&b.date)))
            .map(|d| day_record("Earliest start", "o'clock", hours(d.first_word), d.date)),
        days.iter()
            .max_by(|a, b| a.last_word.cmp(&b.last_word).then(b.date.cmp(&a.date)))
            .map(|d| day_record("Latest finish", "o'clock", hours(d.last_word), d.date)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::{spoken, voice, with_voices};

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn record<'a>(records: &'a [Record], name: &str) -> &'a Record {
        records.iter().find(|r| r.name == name).unwrap()
    }

    #[test]
    fn the_longest_meeting_is_named_with_its_minutes() {
        let mut long = spoken("2026-03-02T10:00:00Z", "2026-03-02T11:30:00Z");
        long.title = "Offsite".into();
        let rows = [spoken("2026-03-03T10:00:00Z", "2026-03-03T10:30:00Z"), long];

        let r = record(&report(&rows, &utc()), "Longest meeting").clone();
        assert_eq!(
            (r.value, r.meeting.unwrap().title),
            (90.0, "Offsite".to_string())
        );
    }

    #[test]
    fn your_longest_monologue_ignores_everyone_else() {
        let rows = [with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![
                voice(Voice::Me, 300),
                voice(Voice::Named("Ana".into()), 900),
            ],
        )];

        let records = report(&rows, &utc());
        assert_eq!(
            (
                record(&records, "Longest monologue").who.as_deref(),
                record(&records, "Your longest monologue").value
            ),
            (Some("Ana"), 300.0)
        );
    }

    #[test]
    fn the_busiest_day_sums_its_meetings() {
        let rows = [
            spoken("2026-03-02T09:00:00Z", "2026-03-02T10:00:00Z"),
            spoken("2026-03-02T11:00:00Z", "2026-03-02T12:30:00Z"),
            spoken("2026-03-03T09:00:00Z", "2026-03-03T11:00:00Z"),
        ];

        let r = record(&report(&rows, &utc()), "Busiest day").clone();
        assert_eq!((r.value, r.day), (2.5, NaiveDate::from_ymd_opt(2026, 3, 2)));
    }

    #[test]
    fn back_and_forth_needs_ten_minutes_of_talk() {
        let mut short = spoken("2026-03-02T10:00:00Z", "2026-03-02T10:05:00Z");
        short.transcript.as_mut().unwrap().handoffs = 50;
        let mut long = spoken("2026-03-03T10:00:00Z", "2026-03-03T10:30:00Z");
        long.transcript.as_mut().unwrap().handoffs = 20;

        let r = record(&report(&[short, long], &utc()), "Most back and forth").clone();
        assert_eq!(r.value, 40.0);
    }
}
