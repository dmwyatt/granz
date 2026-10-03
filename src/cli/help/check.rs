//! Runs documented command lines through the parser grans itself uses.
//!
//! Help examples go through [`parses`]; commands named in help prose and in
//! the agent skill go through [`names_a_command`]. Removing or renaming a
//! command or flag therefore fails the tests of whatever still documents it.
//! Only command lines are checked; the prose beside them is not.

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};

use crate::cli::args::Cli;

/// Stands for any top-level command in a documented command line.
const COMMAND_PLACEHOLDER: &str = "<command>";

/// The arguments of a command line written the way a reader would type it.
/// Output redirection belongs to the shell, so it and the file after it are
/// dropped.
fn argv(command_line: &str) -> Result<Vec<String>, String> {
    let words = shlex::split(command_line)
        .ok_or_else(|| format!("`{command_line}` has unbalanced quotes"))?;
    let argv: Vec<String> = words.into_iter().take_while(|word| word != ">").collect();
    match argv.first().map(String::as_str) {
        Some("grans") => Ok(argv),
        _ => Err(format!("`{command_line}` does not run grans")),
    }
}

/// clap reports a request for help or the version as an error, but the
/// command line that asked for it is valid.
const ANSWERED: [ErrorKind; 2] = [ErrorKind::DisplayHelp, ErrorKind::DisplayVersion];

/// What clap reports when a command line stops before its required arguments.
const INCOMPLETE: [ErrorKind; 3] = [
    ErrorKind::MissingRequiredArgument,
    ErrorKind::MissingSubcommand,
    ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
];

fn check(command_line: &str, tolerated: &[ErrorKind]) -> Result<(), String> {
    match Cli::try_parse_from(argv(command_line)?) {
        Ok(_) => Ok(()),
        Err(e) if tolerated.contains(&e.kind()) => Ok(()),
        Err(e) => Err(format!(
            "`{command_line}` does not parse: {}",
            e.to_string().lines().next().unwrap_or_default()
        )),
    }
}

/// Check that an example runs exactly as written.
pub fn parses(command_line: &str) -> Result<(), String> {
    check(command_line, &ANSWERED)
}

/// Check a command line named in prose, where a command often appears
/// without its arguments (`grans grep`). Everything written has to be real;
/// what was left out does not count against it.
pub fn names_a_command(command_line: &str) -> Result<(), String> {
    check(command_line, &[ANSWERED.as_slice(), &INCOMPLETE].concat())
}

/// The grans command lines a text names in backticks, however the text is
/// wrapped. A line using [`COMMAND_PLACEHOLDER`] yields one line per command.
pub fn mentioned_commands(text: &str) -> Vec<String> {
    let unwrapped = text.split_whitespace().collect::<Vec<_>>().join(" ");
    unwrapped
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.starts_with("grans "))
        .flat_map(expand_command_placeholder)
        .collect()
}

fn expand_command_placeholder(command_line: &str) -> Vec<String> {
    if !command_line.contains(COMMAND_PLACEHOLDER) {
        return vec![command_line.to_string()];
    }
    Cli::command()
        .get_subcommands()
        .map(|command| command_line.replace(COMMAND_PLACEHOLDER, command.get_name()))
        .collect()
}

#[test]
fn a_current_command_line_parses() {
    parses("grans sync --all").unwrap();
}

#[test]
fn removed_flags_fail() {
    // The two that drifted in the hand-maintained skill (#150): `--embed`
    // exists only on `sync transcripts`, and search no longer takes `-y`.
    for command_line in ["grans sync --embed", "grans search \"standup\" -y"] {
        let problem = parses(command_line).expect_err(command_line);
        assert!(
            problem.contains(command_line),
            "the failure should name the command line: {problem}"
        );
    }
}

#[test]
fn an_unknown_command_fails_even_when_asking_for_help() {
    assert!(parses("grans frobnicate --help").is_err());
}

#[test]
fn asking_for_help_or_the_version_parses() {
    for command_line in ["grans --help", "grans sync --help", "grans --version"] {
        parses(command_line).unwrap();
    }
}

#[test]
fn quoted_arguments_stay_whole() {
    assert_eq!(
        argv(r#"grans grep '"pricing review"' --speaker "Jane Doe""#).unwrap(),
        [
            "grans",
            "grep",
            "\"pricing review\"",
            "--speaker",
            "Jane Doe"
        ]
    );
}

#[test]
fn output_redirection_is_left_to_the_shell() {
    assert_eq!(
        argv("grans skill > ~/skills/grans/SKILL.md").unwrap(),
        ["grans", "skill"]
    );
}

#[test]
fn prose_may_name_a_command_without_its_arguments() {
    for command_line in ["grans grep", "grans browse people", "grans sync --all"] {
        names_a_command(command_line).unwrap();
    }
    assert!(parses("grans grep").is_err());
}

#[test]
fn prose_may_not_name_a_command_or_flag_that_does_not_exist() {
    for command_line in [
        "grans frobnicate",
        "grans search --semantic",
        "grans sync --embed",
    ] {
        assert!(names_a_command(command_line).is_err(), "{command_line}");
    }
}

#[test]
fn a_line_that_does_not_run_grans_fails() {
    assert!(parses("jq '.meetings[].id'").is_err());
}

#[test]
fn mentioned_commands_are_the_backticked_grans_command_lines() {
    let text = "`grans` reads embeddings; `--fast` skips a stage. Run\n  `grans embed\n  -y` or `grans sync --all`.";
    assert_eq!(
        mentioned_commands(text),
        ["grans embed -y", "grans sync --all"]
    );
}

#[test]
fn the_command_placeholder_stands_for_every_command() {
    let lines = mentioned_commands("Run `grans <command> --help`.");
    assert!(lines.contains(&"grans search --help".to_string()));
    assert!(lines.contains(&"grans sync --help".to_string()));
    assert!(!lines.iter().any(|line| line.contains(COMMAND_PLACEHOLDER)));
}
