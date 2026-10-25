//! Named speakers: what Granola's detected speaker names add on top of the
//! two audio channels, for meetings recorded since it started naming them.
//!
//! A detected name is a display name, not an identity, so a person here is
//! a name compared without case or extra spaces. Invitees are matched to
//! speakers the same way, through the name Granola's people list gives
//! them. Only meetings with at least one named speaker count, since older
//! meetings say nothing about who spoke.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use serde::Serialize;

use super::calendar::Rsvp;
use super::overview::share;
use super::{MeetingRow, TranscriptFacts, Voice, median};

/// A name as compared: lowercased, with runs of whitespace collapsed.
fn key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpeakerRollup {
    /// The spelling seen most often.
    pub name: String,
    /// Meetings they were heard in.
    pub spoke_in: usize,
    pub chars: u64,
    /// Their characters over everyone's, in the meetings they were heard in.
    pub share_when_heard: Option<f64>,
    /// Meetings with speaker names they were invited to and did not decline.
    pub invited_to: usize,
    pub invited_not_heard: usize,
    pub heard_not_invited: usize,
    /// Times the user spoke right after them.
    pub you_answered: u32,
    /// Times they spoke right after the user.
    pub answered_you: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesVoices {
    pub id: String,
    pub title: String,
    pub meetings: usize,
    /// Whoever talked most across the series' named meetings, the user
    /// included.
    pub top_speaker: String,
    pub top_share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvennessPeriod {
    pub month: NaiveDate,
    pub meetings: usize,
    /// Median over meetings: 1 when everyone heard talked equally, near 0
    /// when one voice did nearly all of it.
    pub typical_evenness: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeakersReport {
    pub meetings_with_names: usize,
    pub since: Option<DateTime<Utc>>,
    /// Most characters first, then invitees who were never heard.
    pub people: Vec<SpeakerRollup>,
    /// Most named meetings first.
    pub series: Vec<SeriesVoices>,
    pub evenness: Vec<EvennessPeriod>,
}

fn named(facts: &TranscriptFacts) -> impl Iterator<Item = (&str, u64)> {
    facts.voices.iter().filter_map(|v| match &v.voice {
        Voice::Named(name) => Some((name.as_str(), v.chars)),
        _ => None,
    })
}

/// Normalized entropy of the talk among the user and the named speakers.
/// `None` with fewer than two of them.
pub fn evenness(facts: &TranscriptFacts) -> Option<f64> {
    let chars: Vec<f64> = facts
        .voices
        .iter()
        .filter(|v| matches!(v.voice, Voice::Me | Voice::Named(_)) && v.chars > 0)
        .map(|v| v.chars as f64)
        .collect();
    if chars.len() < 2 {
        return None;
    }
    let total: f64 = chars.iter().sum();
    let entropy: f64 = chars
        .iter()
        .map(|c| {
            let p = c / total;
            -p * p.ln()
        })
        .sum();
    Some(entropy / (chars.len() as f64).ln())
}

pub fn report(rows: &[MeetingRow], tz: &FixedOffset) -> SpeakersReport {
    let meetings: Vec<(&MeetingRow, &TranscriptFacts)> = rows
        .iter()
        .filter_map(|r| Some((r, r.transcript.as_ref()?)))
        .filter(|(_, t)| t.named_speakers() > 0)
        .collect();

    SpeakersReport {
        meetings_with_names: meetings.len(),
        since: meetings.iter().map(|(r, _)| r.created_at).min(),
        people: people(&meetings),
        series: series(&meetings),
        evenness: evenness_by_month(&meetings, tz),
    }
}

/// The person a heard name refers to in one meeting: the invitee with the
/// same name, or else the one invitee sharing its first name (Granola often
/// hears a short form of the invited name), or else the name itself.
fn resolve(heard: &str, invitees: &[&str]) -> String {
    let heard = key(heard);
    if invitees.iter().any(|i| key(i) == heard) {
        return heard;
    }
    let first = |name: &str| name.split(' ').next().unwrap_or_default().to_string();
    let mut same_first = invitees
        .iter()
        .map(|i| key(i))
        .filter(|i| first(i) == first(&heard));
    match (same_first.next(), same_first.next()) {
        (Some(only), None) => only,
        _ => heard,
    }
}

fn people(meetings: &[(&MeetingRow, &TranscriptFacts)]) -> Vec<SpeakerRollup> {
    let mut rollups: BTreeMap<String, SpeakerRollup> = BTreeMap::new();
    // Everyone's characters in the meetings each person was heard in.
    let mut heard_totals: BTreeMap<String, u64> = BTreeMap::new();
    let mut spellings: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut spell = |person: &str, name: &str| {
        *spellings
            .entry(person.to_string())
            .or_default()
            .entry(name.to_string())
            .or_default() += 1;
    };

    for (row, facts) in meetings {
        let total: u64 = facts.voices.iter().map(|v| v.chars).sum();
        let invitees: Vec<&str> = row
            .invitees
            .iter()
            .filter(|i| i.response != Some(Rsvp::Declined))
            .filter_map(|i| i.name.as_deref())
            .collect();
        let invited: BTreeSet<String> = invitees.iter().map(|n| key(n)).collect();
        for name in &invitees {
            spell(&key(name), name);
        }
        let mut heard: BTreeMap<String, u64> = BTreeMap::new();
        for (name, chars) in named(facts) {
            let person = resolve(name, &invitees);
            spell(&person, name);
            *heard.entry(person).or_default() += chars;
        }

        for (k, chars) in &heard {
            let r = rollups.entry(k.clone()).or_default();
            r.spoke_in += 1;
            r.chars += chars;
            *heard_totals.entry(k.clone()).or_default() += total;
            if !invited.contains(k) {
                r.heard_not_invited += 1;
            }
        }
        for k in &invited {
            let r = rollups.entry(k.clone()).or_default();
            r.invited_to += 1;
            if !heard.contains_key(k) {
                r.invited_not_heard += 1;
            }
        }
        for turn in &facts.turns {
            match (&turn.from, &turn.to) {
                (Voice::Named(n), Voice::Me) => {
                    rollups
                        .entry(resolve(n, &invitees))
                        .or_default()
                        .you_answered += turn.count
                }
                (Voice::Me, Voice::Named(n)) => {
                    rollups
                        .entry(resolve(n, &invitees))
                        .or_default()
                        .answered_you += turn.count
                }
                _ => {}
            }
        }
    }

    let mut people: Vec<SpeakerRollup> = rollups
        .into_iter()
        .map(|(k, mut r)| {
            let total = heard_totals.get(&k).copied().unwrap_or(0);
            r.share_when_heard = share(r.chars, total - r.chars);
            r.name = spellings
                .get(&k)
                .and_then(|s| s.iter().max_by_key(|(_, n)| **n))
                .map_or(k, |(name, _)| name.clone());
            r
        })
        .collect();
    people.sort_by(|a, b| b.chars.cmp(&a.chars).then_with(|| a.name.cmp(&b.name)));
    people
}

fn series(meetings: &[(&MeetingRow, &TranscriptFacts)]) -> Vec<SeriesVoices> {
    let mut by_series: BTreeMap<&str, Vec<&(&MeetingRow, &TranscriptFacts)>> = BTreeMap::new();
    for m in meetings {
        if let Some(id) = &m.0.series {
            by_series.entry(id).or_default().push(m);
        }
    }
    let mut out: Vec<SeriesVoices> = by_series
        .into_iter()
        .filter_map(|(id, ms)| {
            let mut chars: BTreeMap<String, u64> = BTreeMap::new();
            for (_, facts) in &ms {
                for v in &facts.voices {
                    let who = match &v.voice {
                        Voice::Me => "You".to_string(),
                        Voice::Named(n) => n.clone(),
                        Voice::Others | Voice::Unknown => continue,
                    };
                    *chars.entry(who).or_default() += v.chars;
                }
            }
            let total: u64 = chars.values().sum();
            let (top, top_chars) = chars.into_iter().max_by_key(|(_, c)| *c)?;
            let latest = ms.iter().max_by_key(|(r, _)| r.created_at)?;
            Some(SeriesVoices {
                id: id.to_string(),
                title: latest.0.title.clone(),
                meetings: ms.len(),
                top_speaker: top,
                top_share: top_chars as f64 / total as f64,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.meetings
            .cmp(&a.meetings)
            .then_with(|| a.title.cmp(&b.title))
    });
    out
}

fn evenness_by_month(
    meetings: &[(&MeetingRow, &TranscriptFacts)],
    tz: &FixedOffset,
) -> Vec<EvennessPeriod> {
    let mut months: BTreeMap<NaiveDate, Vec<f64>> = BTreeMap::new();
    for (row, facts) in meetings {
        let month = row
            .created_at
            .with_timezone(tz)
            .date_naive()
            .with_day(1)
            .expect("every month has a first day");
        let values = months.entry(month).or_default();
        values.extend(evenness(facts));
    }
    months
        .into_iter()
        .map(|(month, values)| EvennessPeriod {
            month,
            meetings: values.len(),
            typical_evenness: median(values.iter().map(|v| (v * 1000.0).round() as i64))
                .map(|v| v as f64 / 1000.0),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Invitee;
    use crate::stats::calendar::Rsvp;
    use crate::stats::fixtures::{invitee, spoken, voice, with_voices};
    use crate::stats::transcript::Turn;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn ana(chars: u64) -> crate::stats::transcript::VoiceRow {
        voice(Voice::Named("Ana Ruiz".into()), chars)
    }

    fn invited(name: &str) -> Invitee {
        Invitee {
            name: Some(name.to_string()),
            ..invitee(
                &format!("{}@example.com", key(name).replace(' ', ".")),
                Some(Rsvp::Accepted),
            )
        }
    }

    #[test]
    fn invitee_and_speaker_match_without_case_or_spacing() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![ana(100)],
        );
        row.invitees = vec![invited("ana  RUIZ")];

        let p = &report(&[row], &utc()).people[0];
        assert_eq!(
            (p.invited_to, p.heard_not_invited, p.invited_not_heard),
            (1, 0, 0)
        );
    }

    #[test]
    fn a_short_name_matches_the_one_invitee_with_that_first_name() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![voice(Voice::Named("Ana".into()), 100)],
        );
        row.invitees = vec![invited("Ana Ruiz"), invited("Bo Lind")];

        let people = report(&[row], &utc()).people;
        let ana = people.iter().find(|p| p.spoke_in == 1).unwrap();
        assert_eq!((ana.invited_to, ana.heard_not_invited), (1, 0));
    }

    #[test]
    fn a_short_name_shared_by_two_invitees_stays_unmatched() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![voice(Voice::Named("Ana".into()), 100)],
        );
        row.invitees = vec![invited("Ana Ruiz"), invited("Ana Holm")];

        let people = report(&[row], &utc()).people;
        let heard = people.iter().find(|p| p.spoke_in == 1).unwrap();
        assert_eq!((heard.name.as_str(), heard.heard_not_invited), ("Ana", 1));
    }

    #[test]
    fn an_invitee_who_never_spoke_is_counted_as_not_heard() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![ana(100)],
        );
        row.invitees = vec![invited("Bo Lind")];

        let people = report(&[row], &utc()).people;
        let bo = people.iter().find(|p| p.name == "Bo Lind").unwrap();
        assert_eq!((bo.invited_not_heard, bo.spoke_in), (1, 0));
    }

    #[test]
    fn meetings_without_speaker_names_are_left_out() {
        let row = with_voices(
            spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
            vec![voice(Voice::Others, 100)],
        );

        assert_eq!(report(&[row], &utc()).meetings_with_names, 0);
    }

    #[test]
    fn replies_come_from_the_turns() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![voice(Voice::Me, 10), ana(100)],
        );
        row.transcript.as_mut().unwrap().turns = vec![
            Turn {
                from: Voice::Named("Ana Ruiz".into()),
                to: Voice::Me,
                count: 3,
            },
            Turn {
                from: Voice::Me,
                to: Voice::Named("Ana Ruiz".into()),
                count: 2,
            },
        ];

        let p = &report(&[row], &utc()).people[0];
        assert_eq!((p.you_answered, p.answered_you), (3, 2));
    }

    #[test]
    fn equal_talk_is_perfectly_even() {
        let row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![
                voice(Voice::Me, 50),
                ana(50),
                voice(Voice::Named("Bo".into()), 50),
            ],
        );

        let e = evenness(row.transcript.as_ref().unwrap()).unwrap();
        assert!((e - 1.0).abs() < 1e-9, "{e}");
    }

    #[test]
    fn one_dominant_voice_is_uneven() {
        let row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![voice(Voice::Me, 1), ana(999)],
        );

        assert!(evenness(row.transcript.as_ref().unwrap()).unwrap() < 0.05);
    }

    #[test]
    fn a_series_names_whoever_talked_most() {
        let mut row = with_voices(
            spoken("2026-08-03T10:00:00Z", "2026-08-03T10:30:00Z"),
            vec![voice(Voice::Me, 25), ana(75)],
        );
        row.series = Some("s1".into());

        let s = &report(&[row], &utc()).series[0];
        assert_eq!((s.top_speaker.as_str(), s.top_share), ("Ana Ruiz", 0.75));
    }
}
