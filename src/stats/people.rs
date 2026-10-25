//! People: who the user meets with, one-on-one and in groups, who fills
//! their calendar, and how invitees answer invitations.
//!
//! A person is an invitee's email. Being invited is not attending, so
//! "meetings with" someone means meetings they were invited to and did not
//! decline. A one-on-one is a meeting with exactly one such invitee, which
//! makes the system channel that person for the whole history, with or
//! without speaker names.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::calendar::Rsvp;
use super::overview::share;
use super::{MeetingRow, median};

/// Meetings with more invitees than this say little about who works
/// together, and would swamp the pairs with all-hands meetings.
pub const MAX_PAIR_MEETING_SIZE: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Responses {
    pub accepted: usize,
    pub declined: usize,
    pub tentative: usize,
    pub needs_action: usize,
}

impl Responses {
    fn add(&mut self, response: Option<Rsvp>) {
        match response {
            Some(Rsvp::Accepted) => self.accepted += 1,
            Some(Rsvp::Declined) => self.declined += 1,
            Some(Rsvp::Tentative) => self.tentative += 1,
            Some(Rsvp::NeedsAction) => self.needs_action += 1,
            None => {}
        }
    }

    /// Accepted over all answered or unanswered invitations.
    pub fn accept_rate(&self) -> Option<f64> {
        let total = self.accepted + self.declined + self.tentative + self.needs_action;
        (total > 0).then(|| self.accepted as f64 / total as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonRollup {
    pub email: String,
    /// The name they appear under most often.
    pub name: Option<String>,
    /// Meetings they were invited to and did not decline.
    pub meetings: usize,
    pub hours: f64,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub one_on_ones: usize,
    pub one_on_one_hours: f64,
    /// Median days between one-on-ones.
    pub one_on_one_every_days: Option<f64>,
    pub last_one_on_one: Option<DateTime<Utc>>,
    /// The user's share of the talk in their one-on-ones.
    pub my_share_one_on_one: Option<f64>,
    /// How they answered every invitation, declined ones included.
    pub responses: Responses,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrganizerRollup {
    pub email: String,
    pub name: Option<String>,
    pub is_me: bool,
    pub meetings: usize,
    pub hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DomainRollup {
    pub domain: String,
    pub people: usize,
    pub meetings: usize,
    pub hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub meetings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeopleReport {
    /// The user's email domain: invitees outside it are external.
    pub my_domain: Option<String>,
    /// Most meetings first.
    pub people: Vec<PersonRollup>,
    /// Most hours first.
    pub organizers: Vec<OrganizerRollup>,
    /// Most meetings first.
    pub domains: Vec<DomainRollup>,
    /// Meetings with at least one invitee outside the user's domain.
    pub external_meetings: usize,
    pub external_hours: f64,
    /// How the user answered the invitations of the meetings they recorded.
    pub my_responses: Responses,
    /// Invitees who share the most meetings of up to
    /// [`MAX_PAIR_MEETING_SIZE`] invitees. Most shared first.
    pub pairs: Vec<Pair>,
}

fn hours(row: &MeetingRow) -> f64 {
    row.transcript
        .as_ref()
        .map_or(0.0, |t| t.span().num_seconds() as f64 / 3600.0)
}

/// Invitees who did not decline.
fn attending(row: &MeetingRow) -> impl Iterator<Item = &super::Invitee> {
    row.invitees
        .iter()
        .filter(|i| i.response != Some(Rsvp::Declined))
}

fn domain(email: &str) -> Option<&str> {
    email.rsplit_once('@').map(|(_, d)| d)
}

pub fn report(rows: &[MeetingRow]) -> PeopleReport {
    let names = names(rows);
    let my_domain = my_domain(rows);
    let external: Vec<&MeetingRow> = rows
        .iter()
        .filter(|r| {
            my_domain.is_some() && attending(r).any(|i| domain(&i.email) != my_domain.as_deref())
        })
        .collect();
    let mut my_responses = Responses::default();
    for row in rows {
        my_responses.add(row.my_response);
    }

    PeopleReport {
        people: people(rows, &names),
        organizers: organizers(rows, &names),
        domains: domains(rows),
        external_meetings: external.len(),
        external_hours: external.iter().map(|r| hours(r)).sum(),
        my_responses,
        pairs: pairs(rows),
        my_domain,
    }
}

/// Each email's most frequent name, ties broken alphabetically.
pub fn names(rows: &[MeetingRow]) -> HashMap<String, String> {
    let mut counts: HashMap<&str, BTreeMap<&str, usize>> = HashMap::new();
    for i in rows.iter().flat_map(|r| &r.invitees) {
        if let Some(name) = &i.name {
            *counts.entry(&i.email).or_default().entry(name).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(email, names)| {
            let best = names
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))?;
            Some((email.to_string(), best.0.to_string()))
        })
        .collect()
}

fn my_domain(rows: &[MeetingRow]) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for email in rows.iter().filter_map(|r| r.my_email.as_deref()) {
        if let Some(d) = domain(email) {
            *counts.entry(d).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(d, _)| d.to_string())
}

fn people(rows: &[MeetingRow], names: &HashMap<String, String>) -> Vec<PersonRollup> {
    let mut invited: BTreeMap<&str, Vec<&MeetingRow>> = BTreeMap::new();
    let mut responses: BTreeMap<&str, Responses> = BTreeMap::new();
    for row in rows {
        for i in &row.invitees {
            responses.entry(&i.email).or_default().add(i.response);
            if i.response != Some(Rsvp::Declined) {
                invited.entry(&i.email).or_default().push(row);
            }
        }
    }

    let mut people: Vec<PersonRollup> = responses
        .into_iter()
        .filter_map(|(email, responses)| {
            let mut meetings = invited.remove(email)?;
            meetings.sort_by_key(|m| m.created_at);
            Some(person(email, names, meetings, responses))
        })
        .collect();
    people.sort_by(|a, b| {
        b.meetings
            .cmp(&a.meetings)
            .then_with(|| a.email.cmp(&b.email))
    });
    people
}

fn person(
    email: &str,
    names: &HashMap<String, String>,
    meetings: Vec<&MeetingRow>,
    responses: Responses,
) -> PersonRollup {
    let one_on_ones: Vec<&&MeetingRow> = meetings.iter().filter(|m| m.invited() == 1).collect();
    let gaps = one_on_ones
        .windows(2)
        .map(|w| (w[1].created_at - w[0].created_at).num_seconds());
    let (mine, theirs) = one_on_ones
        .iter()
        .filter_map(|m| m.transcript.as_ref())
        .fold((0, 0), |(mine, theirs), t| {
            (mine + t.my_chars(), theirs + t.others_chars())
        });

    PersonRollup {
        email: email.to_string(),
        name: names.get(email).cloned(),
        meetings: meetings.len(),
        hours: meetings.iter().map(|m| hours(m)).sum(),
        first: meetings[0].created_at,
        last: meetings[meetings.len() - 1].created_at,
        one_on_ones: one_on_ones.len(),
        one_on_one_hours: one_on_ones.iter().map(|m| hours(m)).sum(),
        one_on_one_every_days: median(gaps).map(|s| s as f64 / 86400.0),
        last_one_on_one: one_on_ones.last().map(|m| m.created_at),
        my_share_one_on_one: share(mine, theirs),
        responses,
    }
}

fn organizers(rows: &[MeetingRow], names: &HashMap<String, String>) -> Vec<OrganizerRollup> {
    let mut by_email: BTreeMap<&str, OrganizerRollup> = BTreeMap::new();
    for row in rows {
        let Some(email) = &row.organizer else {
            continue;
        };
        let o = by_email.entry(email).or_insert_with(|| OrganizerRollup {
            email: email.clone(),
            name: names.get(email).cloned(),
            is_me: row.organized_by_me,
            meetings: 0,
            hours: 0.0,
        });
        o.is_me |= row.organized_by_me;
        o.meetings += 1;
        o.hours += hours(row);
    }
    let mut organizers: Vec<OrganizerRollup> = by_email.into_values().collect();
    organizers.sort_by(|a, b| {
        b.hours
            .total_cmp(&a.hours)
            .then_with(|| a.email.cmp(&b.email))
    });
    organizers
}

fn domains(rows: &[MeetingRow]) -> Vec<DomainRollup> {
    let mut people: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut meetings: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
    for row in rows {
        let here: BTreeSet<&str> = attending(row).filter_map(|i| domain(&i.email)).collect();
        for i in attending(row) {
            if let Some(d) = domain(&i.email) {
                people.entry(d).or_default().insert(&i.email);
            }
        }
        for d in here {
            let m = meetings.entry(d).or_default();
            m.0 += 1;
            m.1 += hours(row);
        }
    }
    let mut domains: Vec<DomainRollup> = meetings
        .into_iter()
        .map(|(domain, (meetings, hours))| DomainRollup {
            domain: domain.to_string(),
            people: people.get(domain).map_or(0, BTreeSet::len),
            meetings,
            hours,
        })
        .collect();
    domains.sort_by(|a, b| {
        b.meetings
            .cmp(&a.meetings)
            .then_with(|| a.domain.cmp(&b.domain))
    });
    domains
}

fn pairs(rows: &[MeetingRow]) -> Vec<Pair> {
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.invited() <= MAX_PAIR_MEETING_SIZE) {
        let emails: BTreeSet<&str> = attending(row).map(|i| i.email.as_str()).collect();
        let emails: Vec<&str> = emails.into_iter().collect();
        for (n, a) in emails.iter().enumerate() {
            for b in &emails[n + 1..] {
                *counts.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut pairs: Vec<Pair> = counts
        .into_iter()
        .map(|((a, b), meetings)| Pair {
            a: a.to_string(),
            b: b.to_string(),
            meetings,
        })
        .collect();
    pairs.sort_by(|x, y| {
        y.meetings
            .cmp(&x.meetings)
            .then_with(|| (&x.a, &x.b).cmp(&(&y.a, &y.b)))
    });
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::{invitee, spoken, untranscribed, voice, with_voices};
    use crate::stats::{Invitee, Voice};

    fn with(mut row: MeetingRow, invitees: Vec<Invitee>) -> MeetingRow {
        row.invitees = invitees;
        row
    }

    fn accepted(email: &str) -> Invitee {
        invitee(email, Some(Rsvp::Accepted))
    }

    fn named(email: &str, name: &str) -> Invitee {
        Invitee {
            name: Some(name.to_string()),
            ..accepted(email)
        }
    }

    fn talk(row: MeetingRow, mine: u64, theirs: u64) -> MeetingRow {
        with_voices(
            row,
            vec![voice(Voice::Me, mine), voice(Voice::Others, theirs)],
        )
    }

    #[test]
    fn a_one_on_one_is_a_meeting_with_one_invitee_who_did_not_decline() {
        let rows = [with(
            untranscribed("2026-03-02T10:00:00Z"),
            vec![
                accepted("ana@example.com"),
                invitee("bo@example.com", Some(Rsvp::Declined)),
            ],
        )];

        let ana = &report(&rows).people[0];
        assert_eq!(
            (ana.email.as_str(), ana.one_on_ones),
            ("ana@example.com", 1)
        );
    }

    #[test]
    fn a_decliner_is_counted_in_responses_but_not_meetings() {
        let rows = [
            with(
                untranscribed("2026-03-02T10:00:00Z"),
                vec![accepted("bo@example.com")],
            ),
            with(
                untranscribed("2026-03-09T10:00:00Z"),
                vec![invitee("bo@example.com", Some(Rsvp::Declined))],
            ),
        ];

        let bo = &report(&rows).people[0];
        assert_eq!((bo.meetings, bo.responses.accept_rate()), (1, Some(0.5)));
    }

    #[test]
    fn share_in_one_on_ones_is_pooled_over_them() {
        let rows = [
            talk(
                with(
                    spoken("2026-03-02T10:00:00Z", "2026-03-02T10:30:00Z"),
                    vec![accepted("ana@example.com")],
                ),
                30,
                10,
            ),
            talk(
                with(
                    spoken("2026-03-09T10:00:00Z", "2026-03-09T10:30:00Z"),
                    vec![accepted("ana@example.com")],
                ),
                10,
                50,
            ),
        ];

        assert_eq!(report(&rows).people[0].my_share_one_on_one, Some(0.4));
    }

    #[test]
    fn a_person_is_named_as_they_most_often_appear() {
        let rows = [
            with(
                untranscribed("2026-03-02T10:00:00Z"),
                vec![named("ana@example.com", "Ana")],
            ),
            with(
                untranscribed("2026-03-03T10:00:00Z"),
                vec![named("ana@example.com", "Ana Ruiz")],
            ),
            with(
                untranscribed("2026-03-04T10:00:00Z"),
                vec![named("ana@example.com", "Ana Ruiz")],
            ),
        ];

        assert_eq!(report(&rows).people[0].name.as_deref(), Some("Ana Ruiz"));
    }

    #[test]
    fn external_meetings_have_an_invitee_outside_my_domain() {
        let mut internal = with(
            untranscribed("2026-03-02T10:00:00Z"),
            vec![accepted("ana@example.com")],
        );
        internal.my_email = Some("me@example.com".into());
        let mut external = with(
            untranscribed("2026-03-03T10:00:00Z"),
            vec![accepted("ana@example.com"), accepted("vic@vendor.test")],
        );
        external.my_email = Some("me@example.com".into());

        let r = report(&[internal, external]);
        assert_eq!(
            (r.my_domain.as_deref(), r.external_meetings),
            (Some("example.com"), 1)
        );
    }

    #[test]
    fn pairs_skip_large_meetings() {
        let team: Vec<Invitee> = (0..11)
            .map(|n| accepted(&format!("p{n}@example.com")))
            .collect();
        let rows = [
            with(untranscribed("2026-03-02T10:00:00Z"), team),
            with(
                untranscribed("2026-03-03T10:00:00Z"),
                vec![accepted("p0@example.com"), accepted("p1@example.com")],
            ),
        ];

        assert_eq!(
            report(&rows).pairs,
            vec![Pair {
                a: "p0@example.com".into(),
                b: "p1@example.com".into(),
                meetings: 1
            }]
        );
    }

    #[test]
    fn organizers_are_ranked_by_hours() {
        let mut short = spoken("2026-03-02T10:00:00Z", "2026-03-02T10:15:00Z");
        short.organizer = Some("ana@example.com".into());
        let mut long = spoken("2026-03-03T10:00:00Z", "2026-03-03T11:00:00Z");
        long.organizer = Some("me@example.com".into());
        long.organized_by_me = true;

        let organizers: Vec<_> = report(&[short, long])
            .organizers
            .into_iter()
            .map(|o| (o.email, o.is_me))
            .collect();
        assert_eq!(
            organizers,
            vec![
                ("me@example.com".to_string(), true),
                ("ana@example.com".to_string(), false)
            ]
        );
    }
}
