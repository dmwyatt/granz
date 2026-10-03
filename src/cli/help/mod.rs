//! What `--help` prints after a command's options: examples and notes.
//!
//! This is where grans's usage documentation lives, next to the clap doc
//! comments in `cli::args`. Each command points at its entry here with
//! `after_long_help`, which clap prints for `--help` and leaves out of `-h`.
//!
//! Examples are data rather than prose so that a test can run every one of
//! them through the real parser (see `check`).

mod admin;
mod browse;
mod meetings;
mod overview;
mod setup;
mod sync;

#[cfg(test)]
pub(crate) mod check;
#[cfg(test)]
mod tests;

use clap::builder::styling::Style;

use overview::OVERVIEW;

/// How clap styles its own headings (`Usage:`, `Options:`), so the headings
/// here match them. clap strips the styling when output is not a terminal.
const HEADING: Style = Style::new().bold().underline();

const INDENT: &str = "  ";

/// A command line a reader can run, and what it does.
pub struct Example {
    pub command: &'static str,
    pub about: &'static str,
}

/// A titled block of prose. `lines` are printed as written, so they are
/// wrapped by hand; an empty line separates paragraphs.
pub struct Section {
    pub title: &'static str,
    pub lines: &'static [&'static str],
}

/// How the date filters read their values, shown by every command that
/// filters meetings or events by date.
const DATES: Section = Section {
    title: "Dates",
    lines: &[
        "--date takes today, yesterday, this-week, last-week, this-month, or",
        "last-month, and wins over --from and --to. Those take a date",
        "(2026-01-15), a timestamp, or an age such as 3d, 2w, or 1m.",
        "",
        "A value grans cannot read is ignored rather than rejected, so a",
        "misspelled filter returns everything.",
    ],
};

/// The help shown after one command's options.
pub struct CommandHelp {
    /// The command's path below `grans`, e.g. `sync transcripts`.
    pub path: &'static str,
    pub examples: &'static [Example],
    pub sections: &'static [Section],
}

/// A task that takes more than one command.
pub struct Workflow {
    pub name: &'static str,
    pub steps: &'static [Example],
    pub lines: &'static [&'static str],
}

/// The help shown after the command list of `grans --help`.
pub struct Overview {
    /// What someone arrives wanting to do, and the command that does it.
    pub tasks: &'static [Example],
    pub workflows: &'static [Workflow],
    pub closing: &'static [&'static str],
}

impl CommandHelp {
    fn render(&self, heading: Style) -> String {
        let examples: Vec<_> = self.examples.iter().map(|e| (e.command, e.about)).collect();
        let mut blocks = vec![titled("Examples", &two_columns(&examples, INDENT), heading)];
        blocks.extend(
            self.sections
                .iter()
                .map(|section| titled(section.title, &indented(section.lines, INDENT), heading)),
        );
        blocks.join("\n\n")
    }
}

impl Workflow {
    fn render(&self) -> String {
        let step_indent = INDENT.repeat(2);
        let steps: Vec<_> = self.steps.iter().map(|s| (s.command, s.about)).collect();
        let mut lines = vec![format!("{INDENT}{}", self.name)];
        lines.extend(two_columns(&steps, &step_indent));
        lines.extend(indented(self.lines, &step_indent));
        lines.join("\n")
    }
}

impl Overview {
    fn render(&self, heading: Style) -> String {
        let tasks: Vec<_> = self.tasks.iter().map(|t| (t.about, t.command)).collect();
        let workflows: Vec<_> = self.workflows.iter().map(Workflow::render).collect();
        [
            titled("Common tasks", &two_columns(&tasks, INDENT), heading),
            titled("Workflows", &[workflows.join("\n\n")], heading),
            self.closing.join("\n"),
        ]
        .join("\n\n")
    }
}

fn titled(title: &str, lines: &[String], heading: Style) -> String {
    format!("{heading}{title}:{heading:#}\n{}", lines.join("\n"))
}

/// Rows of two columns, the second starting past the widest first cell.
fn two_columns(rows: &[(&str, &str)], indent: &str) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(left, _)| left.chars().count())
        .max()
        .unwrap_or_default();
    rows.iter()
        .map(|(left, right)| format!("{indent}{left:<width$}  {right}"))
        .collect()
}

/// Indent each line, leaving the blank ones that separate paragraphs empty.
fn indented(lines: &[&str], indent: &str) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect()
}

/// Every command's help entry.
fn commands() -> impl Iterator<Item = &'static CommandHelp> {
    [
        meetings::COMMANDS,
        sync::COMMANDS,
        setup::COMMANDS,
        browse::COMMANDS,
        admin::COMMANDS,
    ]
    .into_iter()
    .flatten()
}

/// The examples and notes for the command at `path` below `grans`.
pub fn for_command(path: &str) -> String {
    commands()
        .find(|help| help.path == path)
        .unwrap_or_else(|| panic!("no help written for `grans {path}`"))
        .render(HEADING)
}

/// The task map and workflows for `grans --help`.
pub fn overview() -> String {
    OVERVIEW.render(HEADING)
}
