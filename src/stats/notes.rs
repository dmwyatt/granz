//! Notes: the AI summaries Granola writes (panels), the action items in
//! them, and the notes the user typed.

use std::collections::BTreeMap;

use chrono::{Datelike, FixedOffset, NaiveDate};
use serde::Serialize;

use super::MeetingRow;
use super::words::words;

/// One AI summary of a meeting, as stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelRecord {
    pub template_slug: Option<String>,
    pub markdown: String,
}

/// One AI summary, reduced to what statistics use.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Which of Granola's summary templates wrote it. Its history dates
    /// Granola's changes to them.
    pub template: Option<String>,
    pub action_items: Vec<ActionItem>,
}

impl Summary {
    pub fn new(panel: &PanelRecord) -> Self {
        Summary {
            template: panel.template_slug.clone(),
            action_items: action_items(&panel.markdown),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionItem {
    pub text: String,
    /// The name an item was listed under, when the summary groups items by
    /// person ("Sam:" then bullets).
    pub owner: Option<String>,
}

fn is_action_heading(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|c| *c == '#').count();
    let title = line[level..].trim().to_lowercase();
    (level > 0 && (title.contains("next steps") || title.contains("action items"))).then_some(level)
}

fn bullet(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| {
            let digits = line.chars().take_while(char::is_ascii_digit).count();
            (digits > 0)
                .then(|| line[digits..].strip_prefix(". "))
                .flatten()
        })?;
    Some(rest.trim())
}

/// Markdown emphasis and checkboxes, removed from an item's text.
fn plain(text: &str) -> String {
    text.trim_start_matches("[ ]")
        .trim_start_matches("[x]")
        .replace("**", "")
        .replace('`', "")
        .trim()
        .to_string()
}

/// The items listed under "Next steps" or "Action items" headings: each
/// top-level bullet is an item, and indented lines below it, sub-bullets
/// included, belong to it. An unbulleted line ending in a colon names the
/// owner of the bullets that follow.
pub fn action_items(markdown: &str) -> Vec<ActionItem> {
    let mut items: Vec<ActionItem> = Vec::new();
    let mut in_section = false;
    let mut owner: Option<String> = None;
    for line in markdown.lines() {
        if line.starts_with('#') {
            in_section = is_action_heading(line).is_some();
            owner = None;
            continue;
        }
        if !in_section || line.trim().is_empty() || line.trim() == "---" {
            continue;
        }
        if let Some(text) = bullet(line) {
            items.push(ActionItem {
                text: plain(text),
                owner: owner.clone(),
            });
        } else if line.starts_with(char::is_whitespace) {
            if let Some(item) = items.last_mut() {
                let more = plain(bullet(line.trim()).unwrap_or(line.trim()));
                item.text = format!("{} {more}", item.text);
            }
        } else if let Some(name) = plain(line).strip_suffix(':') {
            owner = Some(name.trim().to_string());
        }
    }
    items
}

/// Whether an item names `first_name` (in its text or owner), as a whole
/// word, ignoring case.
pub fn names(item: &ActionItem, first_name: &str) -> bool {
    let first = first_name.to_lowercase();
    let said = |text: &str| words(text).iter().any(|w| *w == first);
    said(&item.text) || item.owner.as_deref().is_some_and(said)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesMonth {
    pub month: NaiveDate,
    pub meetings: usize,
    /// Meetings where the user typed notes of their own.
    pub with_typed_notes: usize,
    pub action_items: usize,
    /// Summaries per template.
    pub templates: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesItems {
    pub series: String,
    pub title: String,
    pub meetings: usize,
    pub action_items: usize,
    pub naming_me: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateSpan {
    pub template: String,
    pub summaries: usize,
    pub first: NaiveDate,
    pub last: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MyItem {
    pub meeting: String,
    pub title: String,
    pub day: NaiveDate,
    pub item: ActionItem,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesReport {
    pub months: Vec<NotesMonth>,
    /// In the order each template first appeared.
    pub templates: Vec<TemplateSpan>,
    /// Most action items first.
    pub series: Vec<SeriesItems>,
    /// Items naming the user, newest first.
    pub mine: Vec<MyItem>,
}

const NO_TEMPLATE: &str = "(none)";

pub fn report(rows: &[MeetingRow], tz: &FixedOffset) -> NotesReport {
    let day = |row: &MeetingRow| row.created_at.with_timezone(tz).date_naive();
    let first_name = |row: &MeetingRow| {
        row.my_name
            .as_deref()
            .and_then(|n| n.split_whitespace().next())
            .map(str::to_string)
    };

    let mut months: BTreeMap<NaiveDate, NotesMonth> = BTreeMap::new();
    let mut templates: BTreeMap<String, TemplateSpan> = BTreeMap::new();
    let mut series: BTreeMap<&str, SeriesItems> = BTreeMap::new();
    let mut mine: Vec<MyItem> = Vec::new();

    for row in rows {
        let d = day(row);
        let month = d.with_day(1).expect("every month has a first day");
        let m = months.entry(month).or_insert_with(|| NotesMonth {
            month,
            meetings: 0,
            with_typed_notes: 0,
            action_items: 0,
            templates: BTreeMap::new(),
        });
        m.meetings += 1;
        m.with_typed_notes += usize::from(row.typed_note_chars > 0);
        let items: Vec<&ActionItem> = row.summaries.iter().flat_map(|s| &s.action_items).collect();
        m.action_items += items.len();
        for s in &row.summaries {
            let name = s
                .template
                .clone()
                .unwrap_or_else(|| NO_TEMPLATE.to_string());
            *m.templates.entry(name.clone()).or_default() += 1;
            let t = templates
                .entry(name.clone())
                .or_insert_with(|| TemplateSpan {
                    template: name,
                    summaries: 0,
                    first: d,
                    last: d,
                });
            t.summaries += 1;
            t.first = t.first.min(d);
            t.last = t.last.max(d);
        }

        let me = first_name(row);
        let naming_me: Vec<&ActionItem> = items
            .iter()
            .copied()
            .filter(|i| me.as_deref().is_some_and(|n| names(i, n)))
            .collect();
        mine.extend(naming_me.iter().map(|item| MyItem {
            meeting: row.id.clone(),
            title: row.title.clone(),
            day: d,
            item: (*item).clone(),
        }));
        if let Some(id) = &row.series {
            let s = series.entry(id).or_insert_with(|| SeriesItems {
                series: id.clone(),
                title: row.title.clone(),
                meetings: 0,
                action_items: 0,
                naming_me: 0,
            });
            s.title = row.title.clone();
            s.meetings += 1;
            s.action_items += items.len();
            s.naming_me += naming_me.len();
        }
    }

    let mut templates: Vec<TemplateSpan> = templates.into_values().collect();
    templates.sort_by(|a, b| {
        a.first
            .cmp(&b.first)
            .then_with(|| a.template.cmp(&b.template))
    });
    let mut series: Vec<SeriesItems> = series
        .into_values()
        .filter(|s| s.action_items > 0)
        .collect();
    series.sort_by(|a, b| {
        b.action_items
            .cmp(&a.action_items)
            .then_with(|| a.title.cmp(&b.title))
    });
    mine.sort_by(|a, b| b.day.cmp(&a.day));

    NotesReport {
        months: months.into_values().collect(),
        templates,
        series,
        mine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::untranscribed;

    #[test]
    fn items_are_the_bullets_under_a_next_steps_heading() {
        let markdown = "### Summary\n- Not an item\n\n### Next Steps\n- **Ship** the beta\n- Review pricing\n\n### Notes\n- Also not";

        let texts: Vec<_> = action_items(markdown).into_iter().map(|i| i.text).collect();
        assert_eq!(texts, vec!["Ship the beta", "Review pricing"]);
    }

    #[test]
    fn indented_lines_continue_the_item_above() {
        let markdown = "# Action Items\n- Draft the plan\n  due Friday\n  - with numbers";

        assert_eq!(
            action_items(markdown)[0].text,
            "Draft the plan due Friday with numbers"
        );
    }

    #[test]
    fn a_line_ending_in_a_colon_names_the_owner() {
        let markdown = "### Next Steps\n**Sam Okafor:**\n- Send the deck\nAlex:\n- Book the room";

        let owners: Vec<_> = action_items(markdown)
            .into_iter()
            .map(|i| i.owner)
            .collect();
        assert_eq!(
            owners,
            vec![Some("Sam Okafor".to_string()), Some("Alex".to_string())]
        );
    }

    #[test]
    fn an_item_names_me_by_first_name_as_a_whole_word() {
        let item = ActionItem {
            text: "Alex to send notes; Alexandra reviews".into(),
            owner: None,
        };

        assert_eq!(
            (names(&item, "Alex"), names(&item, "Alexand")),
            (true, false)
        );
    }

    #[test]
    fn a_month_counts_typed_notes_and_templates() {
        let mut with_notes = untranscribed("2026-03-02T10:00:00Z");
        with_notes.typed_note_chars = 40;
        with_notes.summaries = vec![Summary {
            template: Some("meeting-summary".into()),
            action_items: vec![],
        }];
        let without = untranscribed("2026-03-03T10:00:00Z");

        let m = &report(&[with_notes, without], &FixedOffset::east_opt(0).unwrap()).months[0];
        assert_eq!(
            (
                m.meetings,
                m.with_typed_notes,
                m.templates.get("meeting-summary")
            ),
            (2, 1, Some(&1))
        );
    }

    #[test]
    fn items_naming_me_are_collected_per_series() {
        let mut row = untranscribed("2026-03-02T10:00:00Z");
        row.my_name = Some("Alex Morgan".into());
        row.series = Some("s1".into());
        row.summaries = vec![Summary::new(&PanelRecord {
            template_slug: None,
            markdown: "### Next Steps\n- Alex to draft\n- Sam to review".into(),
        })];

        let r = report(&[row], &FixedOffset::east_opt(0).unwrap());
        assert_eq!(
            (
                r.series[0].action_items,
                r.series[0].naming_me,
                r.mine.len()
            ),
            (2, 1, 1)
        );
    }
}
