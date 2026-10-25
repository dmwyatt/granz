//! Time and load: how many hours meetings take, when they land, and how
//! they break up the day.
//!
//! A meeting occupies its sessions of talk (see
//! [`TranscriptFacts::sessions`](super::TranscriptFacts::sessions)), which
//! for nearly every meeting is its first spoken word to its last. Meetings
//! without a transcript count as meetings but add no hours.
//! Days and hours are in the caller's time zone.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, Timelike, Weekday};
use serde::Serialize;

use super::{MeetingRow, median};

/// The part of the day free time is measured in.
pub const WORKDAY_START: u32 = 9;
pub const WORKDAY_END: u32 = 17;

/// Meetings this close together are back to back. Spoken time starts a
/// minute or two after the scheduled start and ends before the scheduled
/// end, so two adjacent calendar slots leave a few minutes of silence.
pub const BACK_TO_BACK_GAP: Duration = Duration::minutes(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Period {
    Week,
    Month,
}

impl Period {
    /// The first day of the period containing `date`. Weeks start on Monday.
    pub fn start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Period::Week => date - Duration::days(date.weekday().num_days_from_monday().into()),
            Period::Month => date.with_day(1).expect("every month has a first day"),
        }
    }
}

/// One day that had talk in a meeting. A session counts as a meeting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayLoad {
    pub date: NaiveDate,
    pub meetings: usize,
    pub hours: f64,
    /// The day's first spoken word, local time.
    pub first_word: NaiveTime,
    pub last_word: NaiveTime,
    /// The longest stretch of the workday with no meeting.
    pub longest_free_minutes: i64,
    /// The most meetings in a row with no more than [`BACK_TO_BACK_GAP`]
    /// between them.
    pub longest_streak: usize,
}

/// How many invitees who did not decline a meeting had, in buckets.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SizeMix {
    pub none: usize,
    pub one: usize,
    pub two: usize,
    pub three_to_five: usize,
    pub six_to_ten: usize,
    pub eleven_plus: usize,
}

impl SizeMix {
    fn add(&mut self, invited: usize) {
        let bucket = match size_bucket(invited) {
            0 => &mut self.none,
            1 => &mut self.one,
            2 => &mut self.two,
            3 => &mut self.three_to_five,
            4 => &mut self.six_to_ten,
            _ => &mut self.eleven_plus,
        };
        *bucket += 1;
    }

    pub fn counts(&self) -> [usize; 6] {
        [
            self.none,
            self.one,
            self.two,
            self.three_to_five,
            self.six_to_ten,
            self.eleven_plus,
        ]
    }
}

pub const SIZE_LABELS: [&str; 6] = ["none", "1", "2", "3-5", "6-10", "11+"];

/// The index into [`SIZE_LABELS`] for a meeting with `invited` invitees.
pub fn size_bucket(invited: usize) -> usize {
    match invited {
        0..=2 => invited,
        3..=5 => 3,
        6..=10 => 4,
        _ => 5,
    }
}

/// One week or month.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeriodLoad {
    pub start: NaiveDate,
    pub meetings: usize,
    pub hours: f64,
    pub meeting_days: usize,
    /// Medians over the period's meeting days.
    pub typical_first_word: Option<NaiveTime>,
    pub typical_last_word: Option<NaiveTime>,
    pub typical_longest_free_minutes: Option<i64>,
    pub longest_streak: usize,
    pub sizes: SizeMix,
}

/// Hours of meetings falling in each hour of one weekday, summed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeekdayHours {
    pub weekday: Weekday,
    pub hours: [f64; 24],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimeReport {
    pub periods: Vec<PeriodLoad>,
    pub days: Vec<DayLoad>,
    pub weekday_hours: Vec<WeekdayHours>,
}

pub fn report(rows: &[MeetingRow], period: Period, tz: &FixedOffset) -> TimeReport {
    let days = days(rows, tz);
    TimeReport {
        periods: periods(rows, &days, period, tz),
        weekday_hours: weekday_hours(rows, tz),
        days,
    }
}

/// A meeting's sessions of talk, in local time.
fn sessions<'a>(
    row: &'a MeetingRow,
    tz: &'a FixedOffset,
) -> impl Iterator<Item = (DateTime<FixedOffset>, DateTime<FixedOffset>)> + 'a {
    row.transcript
        .iter()
        .flat_map(|t| &t.sessions)
        .map(|s| (s.start.with_timezone(tz), s.end.with_timezone(tz)))
}

fn hours(span: Duration) -> f64 {
    span.num_seconds() as f64 / 3600.0
}

pub fn days(rows: &[MeetingRow], tz: &FixedOffset) -> Vec<DayLoad> {
    let mut by_day: BTreeMap<NaiveDate, Vec<(DateTime<FixedOffset>, DateTime<FixedOffset>)>> =
        BTreeMap::new();
    for span in rows.iter().flat_map(|r| sessions(r, tz)) {
        by_day.entry(span.0.date_naive()).or_default().push(span);
    }
    by_day
        .into_iter()
        .map(|(date, mut spans)| {
            spans.sort();
            DayLoad {
                date,
                meetings: spans.len(),
                hours: spans.iter().map(|(s, e)| hours(*e - *s)).sum(),
                first_word: spans
                    .iter()
                    .map(|(s, _)| s.time())
                    .min()
                    .expect("non-empty"),
                last_word: spans
                    .iter()
                    .map(|(_, e)| e.time())
                    .max()
                    .expect("non-empty"),
                longest_free_minutes: longest_free(date, &spans),
                longest_streak: longest_streak(&spans),
            }
        })
        .collect()
}

/// The longest gap between meetings inside the workday, counting the
/// stretches before the first meeting and after the last.
fn longest_free(date: NaiveDate, spans: &[(DateTime<FixedOffset>, DateTime<FixedOffset>)]) -> i64 {
    let at = |hour: u32| date.and_hms_opt(hour, 0, 0).expect("valid hour");
    let (open, close) = (at(WORKDAY_START), at(WORKDAY_END));
    let mut cursor = open;
    let mut longest = Duration::zero();
    for (start, end) in spans {
        let (start, end) = (start.naive_local().max(open), end.naive_local().min(close));
        if start > cursor {
            longest = longest.max(start.min(close) - cursor);
        }
        cursor = cursor.max(end);
    }
    longest = longest.max(close - cursor.min(close));
    longest.num_minutes()
}

/// `spans` must be sorted by start.
fn longest_streak(spans: &[(DateTime<FixedOffset>, DateTime<FixedOffset>)]) -> usize {
    let mut longest = 0;
    let mut current = 0;
    let mut previous_end: Option<DateTime<FixedOffset>> = None;
    for (start, end) in spans {
        current = match previous_end {
            Some(prev) if *start - prev <= BACK_TO_BACK_GAP => current + 1,
            _ => 1,
        };
        longest = longest.max(current);
        previous_end = Some(previous_end.map_or(*end, |p| p.max(*end)));
    }
    longest
}

pub fn periods(
    rows: &[MeetingRow],
    days: &[DayLoad],
    period: Period,
    tz: &FixedOffset,
) -> Vec<PeriodLoad> {
    let mut out: BTreeMap<NaiveDate, PeriodLoad> = BTreeMap::new();
    for row in rows {
        let start = period.start(row.created_at.with_timezone(tz).date_naive());
        let p = out.entry(start).or_insert_with(|| empty_period(start));
        p.meetings += 1;
        p.sizes.add(row.invited());
        for (s, e) in sessions(row, tz) {
            let start = period.start(s.date_naive());
            out.entry(start)
                .or_insert_with(|| empty_period(start))
                .hours += hours(e - s);
        }
    }

    let mut grouped: BTreeMap<NaiveDate, Vec<&DayLoad>> = BTreeMap::new();
    for day in days {
        grouped.entry(period.start(day.date)).or_default().push(day);
    }
    for (start, ds) in grouped {
        let p = out.entry(start).or_insert_with(|| empty_period(start));
        p.meeting_days = ds.len();
        p.typical_first_word = median(ds.iter().map(|d| d.first_word));
        p.typical_last_word = median(ds.iter().map(|d| d.last_word));
        p.typical_longest_free_minutes = median(ds.iter().map(|d| d.longest_free_minutes));
        p.longest_streak = ds.iter().map(|d| d.longest_streak).max().unwrap_or(0);
    }
    out.into_values().collect()
}

fn empty_period(start: NaiveDate) -> PeriodLoad {
    PeriodLoad {
        start,
        meetings: 0,
        hours: 0.0,
        meeting_days: 0,
        typical_first_word: None,
        typical_last_word: None,
        typical_longest_free_minutes: None,
        longest_streak: 0,
        sizes: SizeMix::default(),
    }
}

pub fn weekday_hours(rows: &[MeetingRow], tz: &FixedOffset) -> Vec<WeekdayHours> {
    let mut grid = [[0.0f64; 24]; 7];
    for (start, end) in rows.iter().flat_map(|r| sessions(r, tz)) {
        let mut cursor = start;
        while cursor < end {
            let next_hour = (cursor + Duration::hours(1))
                .with_minute(0)
                .and_then(|t| t.with_second(0))
                .and_then(|t| t.with_nanosecond(0))
                .expect("on the hour is a valid time");
            let slice_end = next_hour.min(end);
            grid[cursor.weekday().num_days_from_monday() as usize][cursor.hour() as usize] +=
                hours(slice_end - cursor);
            cursor = slice_end;
        }
    }
    [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ]
    .into_iter()
    .zip(grid)
    .map(|(weekday, hours)| WeekdayHours { weekday, hours })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::calendar::Rsvp;
    use crate::stats::fixtures::{invitee, spoken, spoken_in, untranscribed};

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn time(s: &str) -> NaiveTime {
        NaiveTime::parse_from_str(s, "%H:%M").unwrap()
    }

    #[test]
    fn a_meeting_is_spread_over_the_hours_it_covers() {
        // 2026-03-02 is a Monday.
        let rows = [spoken("2026-03-02T09:45:00Z", "2026-03-02T10:30:00Z")];

        let monday = &weekday_hours(&rows, &utc())[0];

        assert_eq!(
            (monday.weekday, monday.hours[9], monday.hours[10]),
            (Weekday::Mon, 0.25, 0.5)
        );
    }

    #[test]
    fn longest_free_time_counts_the_gaps_between_and_around_meetings() {
        let rows = [
            spoken("2026-03-02T10:00:00Z", "2026-03-02T11:00:00Z"),
            spoken("2026-03-02T14:00:00Z", "2026-03-02T15:00:00Z"),
        ];

        assert_eq!(days(&rows, &utc())[0].longest_free_minutes, 180);
    }

    #[test]
    fn free_time_after_the_last_meeting_counts() {
        let rows = [spoken("2026-03-02T09:00:00Z", "2026-03-02T10:00:00Z")];

        assert_eq!(days(&rows, &utc())[0].longest_free_minutes, 420);
    }

    #[test]
    fn meetings_outside_the_workday_leave_it_all_free() {
        let rows = [spoken("2026-03-02T18:00:00Z", "2026-03-02T19:00:00Z")];

        assert_eq!(days(&rows, &utc())[0].longest_free_minutes, 480);
    }

    #[test]
    fn a_streak_breaks_at_a_gap_longer_than_ten_minutes() {
        let rows = [
            spoken("2026-03-02T09:00:00Z", "2026-03-02T09:28:00Z"),
            spoken("2026-03-02T09:31:00Z", "2026-03-02T09:58:00Z"),
            spoken("2026-03-02T10:30:00Z", "2026-03-02T11:00:00Z"),
        ];

        assert_eq!(days(&rows, &utc())[0].longest_streak, 2);
    }

    #[test]
    fn days_and_times_are_local() {
        let central = FixedOffset::west_opt(6 * 3600).unwrap();
        let rows = [spoken("2026-03-03T03:00:00Z", "2026-03-03T03:30:00Z")];

        let day = &days(&rows, &central)[0];

        assert_eq!(
            (day.date, day.first_word, day.last_word),
            (
                NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
                time("21:00"),
                time("21:30")
            )
        );
    }

    #[test]
    fn a_month_counts_every_meeting_but_only_spoken_hours() {
        let rows = [
            spoken("2026-03-02T09:00:00Z", "2026-03-02T09:30:00Z"),
            untranscribed("2026-03-20T09:00:00Z"),
        ];
        let days = days(&rows, &utc());

        let march = &periods(&rows, &days, Period::Month, &utc())[0];

        assert_eq!(
            (march.start, march.meetings, march.hours, march.meeting_days),
            (NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), 2, 0.5, 1)
        );
    }

    #[test]
    fn size_buckets_leave_out_decliners() {
        let mut row = untranscribed("2026-03-02T09:00:00Z");
        row.invitees = vec![
            invitee("a@example.com", Some(Rsvp::Accepted)),
            invitee("b@example.com", Some(Rsvp::Declined)),
        ];

        let month = &periods(&[row], &[], Period::Month, &utc())[0];

        assert_eq!(month.sizes.counts(), [0, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn a_resumed_meeting_counts_on_each_day_it_was_talked_in() {
        let row = spoken_in(&[
            ("2026-03-02T09:00:00Z", "2026-03-02T09:30:00Z"),
            ("2026-03-04T09:15:00Z", "2026-03-04T09:45:00Z"),
        ]);

        let hours: Vec<_> = days(&[row], &utc())
            .iter()
            .map(|d| (d.date.day(), d.hours))
            .collect();

        assert_eq!(hours, vec![(2, 0.5), (4, 0.5)]);
    }

    #[test]
    fn weeks_start_on_monday() {
        let sunday = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();

        assert_eq!(
            Period::Week.start(sunday),
            NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()
        );
    }

    #[test]
    fn typical_times_are_medians_over_meeting_days() {
        let rows = [
            spoken("2026-03-02T08:00:00Z", "2026-03-02T16:00:00Z"),
            spoken("2026-03-03T09:00:00Z", "2026-03-03T17:00:00Z"),
            spoken("2026-03-04T10:00:00Z", "2026-03-04T18:00:00Z"),
        ];
        let days = days(&rows, &utc());

        let week = &periods(&rows, &days, Period::Week, &utc())[0];

        assert_eq!(
            (week.typical_first_word, week.typical_last_word),
            (Some(time("09:00")), Some(time("17:00")))
        );
    }
}
