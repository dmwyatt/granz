//! Who was invited to a meeting, merged from its two sources.
//!
//! The calendar event lists every invitee with their response. Granola's
//! people list is the same list minus those who declined, plus anyone added
//! by hand, and it is the only source of a full name for most people (the
//! calendar's `displayName` is usually empty). Merging by email keeps the
//! calendar's responses and Granola's names.

use serde::Serialize;

use super::calendar::{Attendee, CalendarInvite, Rsvp};
use crate::models::DocumentPeople;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Invitee {
    /// Lowercased, so one person has one email across meetings.
    pub email: String,
    /// A display name, which can be spelled differently across meetings.
    pub name: Option<String>,
    /// `None` when the invitee is on Granola's list but not the calendar's.
    pub response: Option<Rsvp>,
    pub organizer: bool,
}

/// Everyone invited other than the user, in calendar order, then anyone
/// only Granola's list names.
pub fn merge(calendar: Option<&CalendarInvite>, people: Option<&DocumentPeople>) -> Vec<Invitee> {
    let granola: Vec<(String, Option<String>)> = people
        .and_then(|p| p.attendees.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let email = a.email.as_deref()?.to_lowercase();
            Some((email, a.full_name().map(str::to_string)))
        })
        .collect();
    let my_email = people
        .and_then(|p| p.creator.as_ref())
        .and_then(|c| c.email.as_deref())
        .map(str::to_lowercase);

    let granola_name = |email: &str| {
        granola
            .iter()
            .find(|(e, _)| e == email)
            .and_then(|(_, name)| name.clone())
    };

    let mut invitees: Vec<Invitee> = calendar
        .map(|c| c.attendees.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|a| !a.is_self && !a.resource)
        .filter_map(|a| from_calendar(a, &granola_name))
        .collect();

    for (email, name) in &granola {
        let known = invitees.iter().any(|i| &i.email == email);
        if !known && my_email.as_ref() != Some(email) {
            invitees.push(Invitee {
                email: email.clone(),
                name: name.clone(),
                response: None,
                organizer: false,
            });
        }
    }
    invitees
}

fn from_calendar(
    attendee: &Attendee,
    granola_name: impl Fn(&str) -> Option<String>,
) -> Option<Invitee> {
    let email = attendee.email.as_deref()?.to_lowercase();
    let name = granola_name(&email).or_else(|| attendee.display_name.clone());
    Some(Invitee {
        email,
        name,
        response: attendee.response_status,
        organizer: attendee.organizer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar(value: serde_json::Value) -> CalendarInvite {
        serde_json::from_value(value).unwrap()
    }

    fn people(value: serde_json::Value) -> DocumentPeople {
        serde_json::from_value(value).unwrap()
    }

    fn named(email: &str, full_name: &str) -> serde_json::Value {
        json!({"email": email, "details": {"person": {"name": {"fullName": full_name}}}})
    }

    #[test]
    fn calendar_response_is_kept_and_granola_supplies_the_name() {
        let cal = calendar(json!({"attendees": [
            {"email": "Pat@Example.com", "responseStatus": "accepted"}
        ]}));
        let ppl = people(json!({"attendees": [named("pat@example.com", "Pat Quinn")]}));

        assert_eq!(
            merge(Some(&cal), Some(&ppl)),
            vec![Invitee {
                email: "pat@example.com".into(),
                name: Some("Pat Quinn".into()),
                response: Some(Rsvp::Accepted),
                organizer: false,
            }]
        );
    }

    #[test]
    fn decliner_missing_from_granola_list_is_still_an_invitee() {
        let cal = calendar(json!({"attendees": [
            {"email": "sam@example.com", "displayName": "Sam R", "responseStatus": "declined"}
        ]}));
        let ppl = people(json!({"attendees": []}));

        assert_eq!(
            merge(Some(&cal), Some(&ppl)),
            vec![Invitee {
                email: "sam@example.com".into(),
                name: Some("Sam R".into()),
                response: Some(Rsvp::Declined),
                organizer: false,
            }]
        );
    }

    #[test]
    fn the_user_and_meeting_rooms_are_not_invitees() {
        let cal = calendar(json!({"attendees": [
            {"email": "me@example.com", "self": true, "responseStatus": "accepted"},
            {"email": "room-4@resource.example.com", "resource": true, "responseStatus": "accepted"}
        ]}));

        assert_eq!(merge(Some(&cal), None), vec![]);
    }

    #[test]
    fn someone_only_on_granola_list_has_no_response() {
        let ppl = people(json!({
            "creator": {"email": "me@example.com"},
            "attendees": [named("lee@example.com", "Lee Park"), {"email": "me@example.com"}]
        }));

        assert_eq!(
            merge(None, Some(&ppl)),
            vec![Invitee {
                email: "lee@example.com".into(),
                name: Some("Lee Park".into()),
                response: None,
                organizer: false,
            }]
        );
    }
}
