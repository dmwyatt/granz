//! `grans stats people`: who you meet with, who organizes, and how
//! invitations are answered.

use chrono::FixedOffset;
use colored::Colorize;

use super::{day, percent};
use crate::stats::people::{MAX_PAIR_MEETING_SIZE, PeopleReport, PersonRollup, Responses};

/// How many organizers, domains and pairs to list.
const SHORT_LIST: usize = 10;

pub(super) fn print(report: &PeopleReport, limit: usize, tz: &FixedOffset) {
    if report.people.is_empty() {
        println!("No invitees found.");
        return;
    }
    print_people(&report.people, limit, tz);
    println!();
    print_organizers(report);
    println!();
    print_domains(report);
    println!();
    println!(
        "{} {}",
        "Your responses to the meetings you recorded:".bold(),
        responses(&report.my_responses)
    );
    println!();
    print_pairs(report);
}

fn name(email: &str, name: Option<&str>) -> String {
    name.map_or(email.to_string(), |n| format!("{n} <{email}>"))
}

fn print_people(people: &[PersonRollup], limit: usize, tz: &FixedOffset) {
    println!("{}", "People you meet with, most meetings first".bold());
    println!(
        "{}",
        format!(
            "{:>8}  {:>6}  {:<10}  {:<10}  {:>5}  {:>5}  {:<10}  {:>4}  {:>7}  person",
            "meetings", "hours", "first", "last", "1:1s", "every", "last 1:1", "you", "accepts"
        )
        .dimmed()
    );
    let shown = if limit == 0 { people.len() } else { limit };
    for p in people.iter().take(shown) {
        println!(
            "{:>8}  {:>6.1}  {:<10}  {:<10}  {:>5}  {:>5}  {:<10}  {:>4}  {:>7}  {}",
            p.meetings,
            p.hours,
            day(p.first, tz),
            day(p.last, tz),
            p.one_on_ones,
            p.one_on_one_every_days
                .map_or("-".to_string(), |d| format!("{d:.0}d")),
            p.last_one_on_one.map_or("-".to_string(), |d| day(d, tz)),
            p.my_share_one_on_one.map_or("-".to_string(), percent),
            p.responses.accept_rate().map_or("-".to_string(), percent),
            name(&p.email, p.name.as_deref()).bold()
        );
    }
    let hidden = people.len().saturating_sub(shown);
    if hidden > 0 {
        println!(
            "{}",
            format!("... and {hidden} more (--limit 0 shows all)").dimmed()
        );
    }
    println!(
        "{}",
        "meetings: invited and did not decline. 1:1s: meetings where they were the\n\
         only such invitee; every: median days between them; you: your share of the\n\
         talk in them. accepts: share of all their invitations they accepted."
            .dimmed()
    );
}

fn print_organizers(report: &PeopleReport) {
    println!("{}", "Who organizes your meetings, most hours first".bold());
    for o in report.organizers.iter().take(SHORT_LIST) {
        let who = if o.is_me {
            "you".to_string()
        } else {
            name(&o.email, o.name.as_deref())
        };
        println!("{:>8}  {:>6.1} h  {}", o.meetings, o.hours, who);
    }
    let total: usize = report.organizers.iter().map(|o| o.meetings).sum();
    let mine: usize = report
        .organizers
        .iter()
        .filter(|o| o.is_me)
        .map(|o| o.meetings)
        .sum();
    if total > 0 {
        println!(
            "{}",
            format!(
                "You organized {mine} of {total} meetings with a known organizer ({}).",
                percent(mine as f64 / total as f64)
            )
            .dimmed()
        );
    }
}

fn print_domains(report: &PeopleReport) {
    println!("{}", "Invitees by email domain, most meetings first".bold());
    for d in report.domains.iter().take(SHORT_LIST) {
        let yours = if report.my_domain.as_deref() == Some(d.domain.as_str()) {
            " (yours)"
        } else {
            ""
        };
        println!(
            "{:>8}  {:>6.1} h  {:>4} people  {}{yours}",
            d.meetings, d.hours, d.people, d.domain
        );
    }
    if report.my_domain.is_some() {
        println!(
            "{}",
            format!(
                "{} meetings ({:.1} hours) included someone outside your domain.",
                report.external_meetings, report.external_hours
            )
            .dimmed()
        );
    }
}

fn responses(r: &Responses) -> String {
    format!(
        "{} accepted, {} tentative, {} unanswered, {} declined",
        r.accepted, r.tentative, r.needs_action, r.declined
    )
}

fn print_pairs(report: &PeopleReport) {
    if report.pairs.is_empty() {
        return;
    }
    println!(
        "{}",
        format!("Often invited together (meetings of up to {MAX_PAIR_MEETING_SIZE} invitees)")
            .bold()
    );
    for p in report.pairs.iter().take(SHORT_LIST) {
        println!("{:>8}  {} + {}", p.meetings, p.a, p.b);
    }
}
