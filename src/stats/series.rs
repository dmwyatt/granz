//! Recurring meetings: each series' hours, cadence, invite list, how
//! punctually it runs, and how much of it the user talks.
//!
//! A series is a recurring meeting, grouped by title (see
//! [`MeetingRow::series`]). Its title is the most recent instance's
//! spelling.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::overview::share;
use super::{MeetingRow, median};

/// How a meeting's talk lined up with its calendar slot, from its first
/// session of talk. A note reopened later for another talk adds sessions
/// that have nothing to do with the slot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Punctuality {
    /// First word minus scheduled start; positive is late.
    pub start_delay_minutes: f64,
    /// End of the first session minus scheduled end; positive ran over.
    pub overrun_minutes: f64,
}

pub fn punctuality(row: &MeetingRow) -> Option<Punctuality> {
    let slot = row.scheduled?;
    let first = row.transcript.as_ref()?.sessions.first()?;
    Some(Punctuality {
        start_delay_minutes: minutes(first.start - slot.start),
        overrun_minutes: minutes(first.end - slot.end),
    })
}

fn minutes(d: Duration) -> f64 {
    d.num_seconds() as f64 / 60.0
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesRollup {
    pub id: String,
    pub title: String,
    pub meetings: usize,
    pub hours: f64,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    /// Median days between consecutive meetings. `None` for a single one.
    pub every_days: Option<f64>,
    /// Invitees who did not decline, at the first and latest meeting.
    pub invited_first: usize,
    pub invited_latest: usize,
    /// Medians over meetings with both a slot and a transcript.
    pub typical_start_delay_minutes: Option<f64>,
    pub typical_overrun_minutes: Option<f64>,
    /// The user's share of characters over the whole series.
    pub my_share: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesReport {
    pub recurring_meetings: usize,
    pub recurring_hours: f64,
    pub one_off_meetings: usize,
    pub one_off_hours: f64,
    /// Most hours first.
    pub series: Vec<SeriesRollup>,
}

fn hours(row: &MeetingRow) -> f64 {
    row.transcript
        .as_ref()
        .map_or(0.0, |t| t.span().num_seconds() as f64 / 3600.0)
}

pub fn report(rows: &[MeetingRow]) -> SeriesReport {
    let mut by_series: BTreeMap<&str, Vec<&MeetingRow>> = BTreeMap::new();
    for row in rows {
        if let Some(id) = &row.series {
            by_series.entry(id).or_default().push(row);
        }
    }
    let (recurring, one_off): (Vec<&MeetingRow>, Vec<&MeetingRow>) =
        rows.iter().partition(|r| r.series.is_some());

    let mut series: Vec<SeriesRollup> = by_series
        .into_iter()
        .map(|(id, meetings)| rollup(id, meetings))
        .collect();
    series.sort_by(|a, b| {
        b.hours
            .total_cmp(&a.hours)
            .then_with(|| a.title.cmp(&b.title))
    });

    SeriesReport {
        recurring_meetings: recurring.len(),
        recurring_hours: recurring.iter().map(|r| hours(r)).sum(),
        one_off_meetings: one_off.len(),
        one_off_hours: one_off.iter().map(|r| hours(r)).sum(),
        series,
    }
}

fn rollup(id: &str, mut meetings: Vec<&MeetingRow>) -> SeriesRollup {
    meetings.sort_by_key(|m| m.created_at);
    let first = meetings[0];
    let latest = meetings[meetings.len() - 1];
    let gaps = meetings
        .windows(2)
        .map(|w| (w[1].created_at - w[0].created_at).num_seconds());
    let timings: Vec<Punctuality> = meetings.iter().filter_map(|m| punctuality(m)).collect();
    let transcripts = meetings.iter().filter_map(|m| m.transcript.as_ref());
    let (mine, others) = transcripts.fold((0, 0), |(mine, others), t| {
        (mine + t.my_chars(), others + t.others_chars())
    });

    SeriesRollup {
        id: id.to_string(),
        title: latest.title.clone(),
        meetings: meetings.len(),
        hours: meetings.iter().map(|m| hours(m)).sum(),
        first: first.created_at,
        last: latest.created_at,
        every_days: median(gaps).map(|s| s as f64 / 86400.0),
        invited_first: first.invited(),
        invited_latest: latest.invited(),
        typical_start_delay_minutes: median_minutes(timings.iter().map(|t| t.start_delay_minutes)),
        typical_overrun_minutes: median_minutes(timings.iter().map(|t| t.overrun_minutes)),
        my_share: share(mine, others),
    }
}

/// Minutes are whole seconds over 60, so ordering their seconds is exact.
fn median_minutes(values: impl Iterator<Item = f64>) -> Option<f64> {
    median(values.map(|m| (m * 60.0).round() as i64)).map(|s| s as f64 / 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Schedule;
    use crate::stats::calendar::Rsvp;
    use crate::stats::fixtures::{invitee, spoken, spoken_in, untranscribed};

    fn in_series(mut row: MeetingRow, series: &str, title: &str) -> MeetingRow {
        row.series = Some(series.to_string());
        row.title = title.to_string();
        row
    }

    fn scheduled(mut row: MeetingRow, start: &str, end: &str) -> MeetingRow {
        row.scheduled = Some(Schedule {
            start: start.parse().unwrap(),
            end: end.parse().unwrap(),
        });
        row
    }

    #[test]
    fn punctuality_compares_the_first_session_with_the_slot() {
        let row = scheduled(
            spoken_in(&[
                ("2026-03-02T10:02:00Z", "2026-03-02T10:35:00Z"),
                ("2026-03-05T09:00:00Z", "2026-03-05T09:20:00Z"),
            ]),
            "2026-03-02T10:00:00Z",
            "2026-03-02T10:30:00Z",
        );

        assert_eq!(
            punctuality(&row),
            Some(Punctuality {
                start_delay_minutes: 2.0,
                overrun_minutes: 5.0
            })
        );
    }

    #[test]
    fn a_series_is_titled_by_its_latest_meeting() {
        let rows = [
            in_series(untranscribed("2026-03-02T10:00:00Z"), "s1", "Design sync"),
            in_series(untranscribed("2026-03-09T10:00:00Z"), "s1", "Design review"),
        ];

        assert_eq!(report(&rows).series[0].title, "Design review");
    }

    #[test]
    fn cadence_is_the_median_gap_between_meetings() {
        let rows = [
            in_series(untranscribed("2026-03-02T10:00:00Z"), "s1", "Sync"),
            in_series(untranscribed("2026-03-09T10:00:00Z"), "s1", "Sync"),
            in_series(untranscribed("2026-03-16T10:00:00Z"), "s1", "Sync"),
            in_series(untranscribed("2026-03-30T10:00:00Z"), "s1", "Sync"),
        ];

        assert_eq!(report(&rows).series[0].every_days, Some(7.0));
    }

    #[test]
    fn invite_list_is_compared_first_to_latest() {
        let mut first = in_series(untranscribed("2026-03-02T10:00:00Z"), "s1", "Sync");
        first.invitees = vec![invitee("a@example.com", Some(Rsvp::Accepted))];
        let mut latest = in_series(untranscribed("2026-03-09T10:00:00Z"), "s1", "Sync");
        latest.invitees = vec![
            invitee("a@example.com", Some(Rsvp::Accepted)),
            invitee("b@example.com", Some(Rsvp::Accepted)),
            invitee("c@example.com", Some(Rsvp::Declined)),
        ];

        let s = &report(&[first, latest]).series[0];
        assert_eq!((s.invited_first, s.invited_latest), (1, 2));
    }

    #[test]
    fn series_are_ordered_by_hours() {
        let rows = [
            in_series(
                spoken("2026-03-02T10:00:00Z", "2026-03-02T10:15:00Z"),
                "short",
                "Standup",
            ),
            in_series(
                spoken("2026-03-02T11:00:00Z", "2026-03-02T12:00:00Z"),
                "long",
                "Planning",
            ),
        ];

        let titles: Vec<_> = report(&rows).series.into_iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["Planning", "Standup"]);
    }

    #[test]
    fn meetings_outside_any_series_are_one_off() {
        let rows = [
            in_series(
                spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
                "s1",
                "Sync",
            ),
            spoken("2026-03-03T10:00:00Z", "2026-03-03T10:15:00Z"),
        ];

        let r = report(&rows);
        assert_eq!(
            (
                r.recurring_meetings,
                r.recurring_hours,
                r.one_off_meetings,
                r.one_off_hours
            ),
            (1, 0.5, 1, 0.25)
        );
    }
}
