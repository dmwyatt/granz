//! `grans stats topics`: what meetings are about, from the embeddings.

use chrono::FixedOffset;
use colored::Colorize;

use super::day;
use crate::stats::topics::TopicsReport;

pub(super) fn print(report: &TopicsReport, tz: &FixedOffset) {
    if report.embedded == 0 {
        println!(
            "None of the {} meetings has an embedding yet; run `grans embed` first.",
            report.meetings
        );
        return;
    }
    println!(
        "{} of {} meetings have an embedding.",
        report.embedded, report.meetings
    );
    if report.embedded < report.meetings {
        println!(
            "{}",
            "`grans embed` covers the rest; until then they are left out.".dimmed()
        );
    }

    println!();
    println!("{}", "Topic groups, largest first".bold());
    for c in &report.clusters {
        let label = if c.label.is_empty() {
            "(no common title words)".to_string()
        } else {
            c.label.join(", ")
        };
        println!(
            "{:>5} meetings  {} to {}  {}",
            c.meetings,
            day(c.first, tz),
            day(c.last, tz),
            label.bold()
        );
    }

    if !report.rehash.is_empty() {
        println!();
        println!(
            "{}",
            "Recurring meetings most like their previous meeting".bold()
        );
        println!(
            "{}",
            format!("{:>7}  {:>6}  {:>6}  series", "typical", "latest", "pairs").dimmed()
        );
        for r in report.rehash.iter().take(10) {
            println!(
                "{:>7.2}  {:>6.2}  {:>6}  {}",
                r.typical_similarity, r.latest_similarity, r.pairs, r.title
            );
        }
    }

    if !report.twins.is_empty() {
        println!();
        println!("{}", "Different series with nearly the same content".bold());
        for t in &report.twins {
            println!("{:>7.2}  {}  +  {}", t.similarity, t.a_title, t.b_title);
        }
    }

    println!();
    println!("{}", "Meetings least like any other".bold());
    for o in &report.outliers {
        println!("{:>7.2}  {}  {}", o.nearest, day(o.created_at, tz), o.title);
    }
    println!(
        "{}",
        "Numbers are cosine similarities between meetings' transcript embeddings:\n\
         higher is more alike. They rank meetings against each other; they are\n\
         not a percentage."
            .dimmed()
    );
}
