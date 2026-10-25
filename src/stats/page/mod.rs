//! The stats page: one self-contained HTML file with the statistics' rows
//! embedded as JSON and the charts drawn in the browser.
//!
//! Everything the page needs is inlined, and its Content-Security-Policy
//! forbids every fetch, so meeting data never leaves the machine. The page
//! filters and aggregates the rows itself; the JS under `js/` mirrors the
//! aggregations in `stats`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use std::collections::{BTreeMap, HashMap};

use super::MeetingRow;
use super::topics::TopicsReport;
use super::words::{MeetingTics, SignatureWord, TICS, TermLife, WordCounter};

const TEMPLATE: &str = include_str!("page.html");
const STYLE: &str = include_str!("page.css");

/// Plot reads d3 from the global, so d3 loads first.
const VENDOR: &[&str] = &[
    include_str!("vendor/d3.min.js"),
    include_str!("vendor/plot.umd.min.js"),
];

/// `core.js` defines what the sections use and `main.js` renders them, so
/// sections go between the two, in page order.
const APP: &[&str] = &[
    include_str!("js/core.js"),
    include_str!("js/overview.js"),
    include_str!("js/time.js"),
    include_str!("js/series.js"),
    include_str!("js/people.js"),
    include_str!("js/talk.js"),
    include_str!("js/speakers.js"),
    include_str!("js/words.js"),
    include_str!("js/topics.js"),
    include_str!("js/main.js"),
];

/// What the page is given. Its field names are the contract with `js/`.
#[derive(Debug, Serialize)]
pub struct Payload<'a> {
    pub generated_at: DateTime<Utc>,
    pub meetings: &'a [MeetingRow],
    pub words: WordsPayload<'a>,
    pub topics: &'a TopicsReport,
}

/// What the page needs from the text pass: each meeting's tics, so they
/// follow the filters, and the vocabulary findings over the whole file.
#[derive(Debug, Serialize)]
pub struct WordsPayload<'a> {
    pub tics: &'static [&'static str],
    pub meeting_tics: &'a HashMap<String, MeetingTics>,
    pub signature: Vec<SignatureWord>,
    pub emerging: Vec<TermLife>,
    /// Full-name mentions per meeting id, then per email, so the page can
    /// tell who was talked about while not invited.
    pub meeting_mentions: &'a HashMap<String, BTreeMap<String, u32>>,
}

impl<'a> WordsPayload<'a> {
    pub fn new(counter: &'a WordCounter, rows: &[MeetingRow]) -> Self {
        let report = counter.report(rows);
        WordsPayload {
            tics: TICS,
            meeting_tics: counter.meeting_tics(),
            signature: report.signature,
            emerging: report.emerging,
            meeting_mentions: counter.mentions(),
        }
    }
}

pub fn render(payload: &Payload) -> Result<String> {
    // The data goes in last, so no meeting text is scanned for a marker.
    Ok(TEMPLATE
        .replacen("/*grans:style*/", STYLE, 1)
        .replacen("/*grans:vendor*/", &VENDOR.join("\n;\n"), 1)
        .replacen("/*grans:app*/", &APP.join("\n"), 1)
        .replacen("/*grans:data*/", &script_safe_json(payload)?, 1))
}

/// JSON that can sit inside a `<script>` element. `<` only occurs inside
/// JSON strings, where `<` means the same character, so escaping all
/// of them rules out a `</script>` in a meeting title ending the element.
fn script_safe_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?.replace('<', "\\u003c"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::MeetingRecord;

    fn page_for(title: &str) -> String {
        let row = MeetingRow::new(
            MeetingRecord {
                id: "m1".into(),
                title: title.into(),
                ..Default::default()
            },
            None,
        );
        let words = WordCounter::new(chrono::FixedOffset::east_opt(0).unwrap(), &[], &[]);
        let rows = [row];
        let topics = crate::stats::topics::report(&rows, &HashMap::new());
        render(&Payload {
            generated_at: "2026-03-02T16:00:00Z".parse().unwrap(),
            meetings: &rows,
            words: WordsPayload::new(&words, &rows),
            topics: &topics,
        })
        .unwrap()
    }

    fn embedded_data(page: &str) -> serde_json::Value {
        let start = page.find(r#"id="grans-data">"#).unwrap() + r#"id="grans-data">"#.len();
        let end = start + page[start..].find("</script>").unwrap();
        serde_json::from_str(&page[start..end]).unwrap()
    }

    #[test]
    fn a_title_cannot_close_the_data_element() {
        let page = page_for("</script><script>alert(1)</script>");

        assert_eq!(
            embedded_data(&page)["meetings"][0]["title"],
            "</script><script>alert(1)</script>"
        );
    }

    #[test]
    fn every_marker_is_filled() {
        let page = page_for("Planning");

        assert!(!page.contains("/*grans:"), "unfilled marker in page");
    }

    #[test]
    fn page_forbids_network_requests() {
        let page = page_for("Planning");

        assert!(page.contains("default-src 'none'"));
    }
}
