//! `grans stats time`: load per period, meeting sizes, and when in the
//! week meetings happen.

use chrono::NaiveTime;
use colored::Colorize;

use crate::stats::time::{Period, PeriodLoad, SIZE_LABELS, TimeReport, WeekdayHours};

pub(super) fn print(report: &TimeReport, by: Period) {
    if report.periods.is_empty() {
        println!("No meetings found.");
        return;
    }
    print_load(&report.periods, by);
    println!();
    print_sizes(&report.periods, by);
    println!();
    print_grid(&report.weekday_hours);
}

fn label(p: &PeriodLoad, by: Period) -> String {
    match by {
        Period::Week => p.start.format("%Y-%m-%d").to_string(),
        Period::Month => p.start.format("%Y-%m").to_string(),
    }
}

fn heading(by: Period) -> &'static str {
    match by {
        Period::Week => "week of",
        Period::Month => "month",
    }
}

fn clock(t: Option<NaiveTime>) -> String {
    t.map_or("-".to_string(), |t| t.format("%H:%M").to_string())
}

fn minutes(m: Option<i64>) -> String {
    m.map_or("-".to_string(), |m| format!("{}h{:02}", m / 60, m % 60))
}

fn print_load(periods: &[PeriodLoad], by: Period) {
    println!(
        "{}",
        format!(
            "{:<10}  {:>8}  {:>6}  {:>4}  {:>10}  {:>9}  {:>9}  {:>12}",
            heading(by),
            "meetings",
            "hours",
            "days",
            "first word",
            "last word",
            "free time",
            "back-to-back"
        )
        .dimmed()
    );
    for p in periods {
        println!(
            "{:<10}  {:>8}  {:>6.1}  {:>4}  {:>10}  {:>9}  {:>9}  {:>12}",
            label(p, by),
            p.meetings,
            p.hours,
            p.meeting_days,
            clock(p.typical_first_word),
            clock(p.typical_last_word),
            minutes(p.typical_longest_free_minutes),
            p.longest_streak
        );
    }
    println!(
        "{}",
        "first word, last word and free time are medians over days with meetings;\n\
         free time is the longest gap between 09:00 and 17:00; back-to-back is the\n\
         most meetings in a row on one day."
            .dimmed()
    );
}

fn print_sizes(periods: &[PeriodLoad], by: Period) {
    println!(
        "{}",
        "Meetings by number of invitees who did not decline".bold()
    );
    let sizes: String = SIZE_LABELS.iter().map(|l| format!("  {l:>5}")).collect();
    println!("{}", format!("{:<10}{sizes}", heading(by)).dimmed());
    for p in periods {
        let counts: String = p
            .sizes
            .counts()
            .iter()
            .map(|c| format!("  {c:>5}"))
            .collect();
        println!("{:<10}{counts}", label(p, by));
    }
}

/// Hours of meetings per weekday and hour, showing only the hours of the
/// day that had any.
fn print_grid(grid: &[WeekdayHours]) {
    let busy: Vec<usize> = (0..24)
        .filter(|h| grid.iter().any(|d| d.hours[*h] >= 0.05))
        .collect();
    let (Some(&first), Some(&last)) = (busy.first(), busy.last()) else {
        return;
    };
    println!("{}", "Hours of meetings by weekday and hour of day".bold());
    let hours: String = (first..=last).map(|h| format!(" {h:>5}")).collect();
    println!("{}", format!("   {hours}").dimmed());
    for day in grid {
        let cells: String = (first..=last)
            .map(|h| {
                let v = day.hours[h];
                if v >= 0.05 {
                    format!(" {v:>5.1}")
                } else {
                    format!(" {:>5}", ".")
                }
            })
            .collect();
        println!("{}{cells}", day.weekday);
    }
}
