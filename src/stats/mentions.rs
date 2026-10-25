//! People mentioned by full name in meetings they were not invited to.
//!
//! Only full names count. First names alone collide with ordinary words
//! ("Will", "Mark") and with other people, so they say too little about
//! who was meant. A person's full name is the one they most often appear
//! under on invitations.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::MeetingRow;
use super::calendar::Rsvp;
use super::words::words;

/// Finds known full names in a run of words.
#[derive(Debug, Default)]
pub struct NameMatcher {
    /// Each name's words, keyed by its first word.
    by_first: HashMap<String, Vec<(usize, Vec<String>)>>,
    /// Emails, indexed as the names were given.
    emails: Vec<String>,
}

impl NameMatcher {
    /// `people` are `(email, full name)`. Names of one word are skipped.
    pub fn new(people: &[(String, String)]) -> Self {
        let mut matcher = NameMatcher::default();
        for (email, name) in people {
            let name_words = words(name);
            if name_words.len() < 2 {
                continue;
            }
            let index = matcher.emails.len();
            matcher.emails.push(email.clone());
            matcher
                .by_first
                .entry(name_words[0].clone())
                .or_default()
                .push((index, name_words));
        }
        matcher
    }

    /// The email of each full name said in `tokens`, once per mention.
    pub fn find<'a>(&'a self, tokens: &'a [String]) -> impl Iterator<Item = &'a str> + 'a {
        tokens.iter().enumerate().flat_map(move |(at, token)| {
            self.by_first
                .get(token)
                .into_iter()
                .flatten()
                .filter(move |(_, name)| tokens[at..].starts_with(name))
                .map(|(index, _)| self.emails[*index].as_str())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AbsentMention {
    pub email: String,
    pub name: String,
    /// Meetings where their name was said and they were not invited, or
    /// declined.
    pub meetings: usize,
    pub mentions: u32,
    pub last: DateTime<Utc>,
}

/// Each person's mentions in meetings they were not attending by
/// invitation, most meetings first. `mentions` is per meeting id, then per
/// email.
pub fn absent(
    rows: &[MeetingRow],
    mentions: &HashMap<String, BTreeMap<String, u32>>,
    names: &HashMap<String, String>,
) -> Vec<AbsentMention> {
    let mut by_person: BTreeMap<&str, AbsentMention> = BTreeMap::new();
    for row in rows {
        let Some(said) = mentions.get(&row.id) else {
            continue;
        };
        for (email, count) in said {
            let invited = row
                .invitees
                .iter()
                .any(|i| &i.email == email && i.response != Some(Rsvp::Declined));
            if invited {
                continue;
            }
            let m = by_person.entry(email).or_insert_with(|| AbsentMention {
                email: email.clone(),
                name: names.get(email).cloned().unwrap_or_else(|| email.clone()),
                meetings: 0,
                mentions: 0,
                last: row.created_at,
            });
            m.meetings += 1;
            m.mentions += count;
            m.last = m.last.max(row.created_at);
        }
    }
    let mut out: Vec<AbsentMention> = by_person.into_values().collect();
    out.sort_by(|a, b| {
        b.meetings
            .cmp(&a.meetings)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::{invitee, untranscribed};

    fn matcher() -> NameMatcher {
        NameMatcher::new(&[
            ("ana@example.com".into(), "Ana Ruiz".into()),
            ("ana.h@example.com".into(), "Ana Holm".into()),
            ("cher@example.com".into(), "Cher".into()),
        ])
    }

    #[test]
    fn a_full_name_is_found_among_other_words() {
        let tokens = words("I asked Ana Ruiz and then ana holm, but not Ana.");
        let matcher = matcher();

        let found: Vec<_> = matcher.find(&tokens).collect();
        assert_eq!(found, vec!["ana@example.com", "ana.h@example.com"]);
    }

    #[test]
    fn a_one_word_name_is_never_matched() {
        let tokens = words("Cher said so");

        assert_eq!(matcher().find(&tokens).count(), 0);
    }

    #[test]
    fn a_mention_counts_only_where_they_were_not_invited() {
        let mut invited = untranscribed("2026-03-02T10:00:00Z");
        invited.id = "with-ana".into();
        invited.invitees = vec![invitee("ana@example.com", Some(Rsvp::Accepted))];
        let mut declined = untranscribed("2026-03-03T10:00:00Z");
        declined.id = "ana-declined".into();
        declined.invitees = vec![invitee("ana@example.com", Some(Rsvp::Declined))];
        let mentions: HashMap<String, BTreeMap<String, u32>> = [
            (
                "with-ana".to_string(),
                BTreeMap::from([("ana@example.com".to_string(), 2)]),
            ),
            (
                "ana-declined".to_string(),
                BTreeMap::from([("ana@example.com".to_string(), 1)]),
            ),
        ]
        .into();

        let absent = absent(&[invited, declined], &mentions, &HashMap::new());
        assert_eq!(
            absent
                .iter()
                .map(|a| (a.meetings, a.mentions))
                .collect::<Vec<_>>(),
            vec![(1, 1)]
        );
    }
}
