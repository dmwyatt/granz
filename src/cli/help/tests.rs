use clap::{Command, CommandFactory};

use super::check::{mentioned_commands, names_a_command, parses};
use super::*;
use crate::cli::args::Cli;

/// Help as it reads once clap has stripped the heading styles.
const PLAIN: Style = Style::new();

/// Every command a reader can see, keyed by its path below `grans`
/// (e.g. `sync transcripts`), built the way grans builds it at run time.
fn visible_commands() -> Vec<(String, Command)> {
    fn collect(parent: &Command, prefix: &str, found: &mut Vec<(String, Command)>) {
        for command in parent.get_subcommands() {
            // clap adds `help` itself, and hidden commands are not documented.
            if command.get_name() == "help" || command.is_hide_set() {
                continue;
            }
            let path = format!("{prefix}{}", command.get_name());
            collect(command, &format!("{path} "), found);
            found.push((path, command.clone()));
        }
    }

    let mut root = Cli::command();
    root.build();
    let mut found = Vec::new();
    collect(&root, "", &mut found);
    found
}

fn long_help(command: &Command) -> String {
    command.clone().render_long_help().to_string()
}

fn short_help(command: &Command) -> String {
    command.clone().render_help().to_string()
}

/// Every example in the help, labelled with where it is shown.
fn all_examples() -> Vec<(String, &'static Example)> {
    let overview = OVERVIEW
        .tasks
        .iter()
        .chain(OVERVIEW.workflows.iter().flat_map(|w| w.steps))
        .map(|example| ("grans --help".to_string(), example));
    let per_command = commands().flat_map(|help| {
        help.examples
            .iter()
            .map(move |example| (format!("grans {} --help", help.path), example))
    });
    overview.chain(per_command).collect()
}

fn assert_none(problems: Vec<String>) {
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn every_example_parses() {
    let examples = all_examples();
    assert!(!examples.is_empty());
    assert_none(
        examples
            .into_iter()
            .filter_map(|(shown_in, example)| {
                parses(example.command)
                    .err()
                    .map(|problem| format!("{shown_in}: {problem}"))
            })
            .collect(),
    );
}

#[test]
fn every_command_named_in_help_text_parses() {
    let mut root = Cli::command();
    root.build();
    let mut helps = vec![("grans --help".to_string(), long_help(&root))];
    helps.extend(
        visible_commands()
            .iter()
            .map(|(path, command)| (format!("grans {path} --help"), long_help(command))),
    );

    assert_none(
        helps
            .iter()
            .flat_map(|(shown_in, text)| {
                mentioned_commands(text)
                    .into_iter()
                    .filter_map(move |command_line| {
                        names_a_command(&command_line)
                            .err()
                            .map(|problem| format!("{shown_in}: {problem}"))
                    })
            })
            .collect(),
    );
}

#[test]
fn every_command_shows_examples_under_long_help_only() {
    for (path, command) in visible_commands() {
        let help = commands()
            .find(|help| help.path == path)
            .unwrap_or_else(|| panic!("no help written for `grans {path}`"));
        assert!(
            !help.examples.is_empty(),
            "`grans {path}` has no examples in its help"
        );
        assert!(
            long_help(&command).contains(&help.render(PLAIN)),
            "`grans {path} --help` does not show the help written for it"
        );
        assert!(
            !short_help(&command).contains("Examples:"),
            "`grans {path} -h` should stay a summary"
        );
    }
}

#[test]
fn every_help_entry_belongs_to_one_command() {
    let paths: Vec<String> = visible_commands()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    for help in commands() {
        assert!(
            paths.iter().any(|path| path == help.path),
            "help is written for `grans {}`, which is not a command",
            help.path
        );
        assert_eq!(
            commands().filter(|other| other.path == help.path).count(),
            1,
            "`grans {}` has more than one help entry",
            help.path
        );
    }
}

#[test]
fn top_level_long_help_has_the_task_map_and_workflows() {
    let root = Cli::command();
    let long = long_help(&root);
    assert!(long.contains(&OVERVIEW.render(PLAIN)));
    assert!(long.contains("Common tasks:"));
    assert!(long.contains("Workflows:"));

    let short = short_help(&root);
    assert!(
        !short.contains("Common tasks:"),
        "`grans -h` should stay a summary"
    );
}

#[test]
fn examples_render_as_an_aligned_table_followed_by_titled_sections() {
    let help = CommandHelp {
        path: "thing",
        examples: &[
            Example {
                command: "grans thing",
                about: "Do the thing",
            },
            Example {
                command: "grans thing --twice",
                about: "Do it twice",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["First line.", "", "Second paragraph."],
        }],
    };

    assert_eq!(
        help.render(PLAIN),
        "\
Examples:
  grans thing          Do the thing
  grans thing --twice  Do it twice

Notes:
  First line.

  Second paragraph."
    );
}

#[test]
fn the_task_map_leads_with_the_task_and_workflows_with_the_command() {
    let overview = Overview {
        tasks: &[
            Example {
                command: "grans search \"<topic>\"",
                about: "Find meetings about a topic",
            },
            Example {
                command: "grans today",
                about: "Today's meetings",
            },
        ],
        workflows: &[Workflow {
            name: "Keep data current",
            steps: &[Example {
                command: "grans sync --all",
                about: "Everything",
            }],
            lines: &["Run it daily."],
        }],
        closing: &["Run `grans <command> --help` for more."],
    };

    assert_eq!(
        overview.render(PLAIN),
        "\
Common tasks:
  Find meetings about a topic  grans search \"<topic>\"
  Today's meetings             grans today

Workflows:
  Keep data current
    grans sync --all  Everything
    Run it daily.

Run `grans <command> --help` for more."
    );
}

#[test]
fn search_grep_and_show_say_how_far_to_trust_ai_notes() {
    for path in ["search", "grep", "show"] {
        let help = for_command(path);
        assert!(
            help.contains("AI notes:") && help.contains("transcript"),
            "`grans {path} --help` should say what AI notes are and to confirm them \
             against the transcript"
        );
    }
}
