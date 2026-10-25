//! `grans stats words`: tics, signature words, and terms over time.

use chrono::FixedOffset;
use colored::Colorize;

use super::day;

use crate::stats::words::{TermLife, WordsReport};

/// How many signature words and new terms to print.
const LISTED: usize = 15;

pub(super) fn print(report: &WordsReport, tz: &FixedOffset) {
    if report.tics_by_month.is_empty() && report.signature.is_empty() {
        println!("No transcript text found.");
        return;
    }
    print_tics(report);
    println!();
    println!("{}", "Words you say far more than everyone else".bold());
    for w in report.signature.iter().take(LISTED) {
        println!(
            "{:>6} you  {:>6} others  {:>5.1}  {}",
            w.mine, w.others, w.score, w.word
        );
    }
    println!(
        "{}",
        "the score is a z-score: above about 2 is unlikely to be chance".dimmed()
    );
    println!();
    println!(
        "{}",
        "Terms first heard after the first tenth of all words, most mentioned first".bold()
    );
    print_lives(&report.emerging[..report.emerging.len().min(LISTED)]);
    if !report.mentioned_while_absent.is_empty() {
        println!();
        println!("{}", "Talked about while not invited, by full name".bold());
        println!(
            "{}",
            format!(
                "{:>8}  {:>8}  {:<10}  person",
                "meetings", "mentions", "last"
            )
            .dimmed()
        );
        for m in report.mentioned_while_absent.iter().take(LISTED) {
            println!(
                "{:>8}  {:>8}  {:<10}  {}",
                m.meetings,
                m.mentions,
                day(m.last, tz),
                m.name
            );
        }
    }
    if !report.terms.is_empty() {
        println!();
        println!("{}", "The terms you asked about".bold());
        print_lives(&report.terms);
        for t in &report.terms {
            println!();
            println!("{}", t.term.bold());
            for (month, n) in &t.monthly {
                println!("  {}  {:>5}", month.format("%Y-%m"), n);
            }
        }
    }
}

fn print_tics(report: &WordsReport) {
    println!("{}", "Your verbal tics, per thousand of your words".bold());
    let names: String = report.tics.iter().map(|t| format!("  {t:>10}")).collect();
    println!(
        "{}",
        format!("{:<7}  {:>7}{names}", "month", "words").dimmed()
    );
    for m in &report.tics_by_month {
        let rates: String = m
            .per_thousand
            .iter()
            .map(|r| format!("  {r:>10.1}"))
            .collect();
        println!("{}  {:>7}{rates}", m.month.format("%Y-%m"), m.my_words);
    }
}

fn print_lives(lives: &[TermLife]) {
    let month =
        |m: Option<chrono::NaiveDate>| m.map_or("-".to_string(), |m| m.format("%Y-%m").to_string());
    println!(
        "{}",
        format!(
            "{:>8}  {:<11}  {:<10}  term",
            "mentions", "first heard", "peak"
        )
        .dimmed()
    );
    for t in lives {
        println!(
            "{:>8}  {:<11}  {:<10}  {}",
            t.mentions,
            month(t.first_month),
            month(t.peak_month),
            t.term
        );
    }
}
