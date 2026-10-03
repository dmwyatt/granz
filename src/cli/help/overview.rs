//! The task map and workflows shown by `grans --help`.
//!
//! Written for someone who knows what they want but not which command does
//! it, so tasks are phrased the way they would be asked, not in terms of
//! grans's data model.

use super::{Example, Overview, Workflow};

pub(super) const OVERVIEW: Overview = Overview {
    tasks: &[
        Example {
            about: "Find meetings about a topic",
            command: r#"grans search "<topic>""#,
        },
        Example {
            about: "List every meeting containing exact words",
            command: r#"grans grep "<words>""#,
        },
        Example {
            about: "Find what one person said",
            command: r#"grans grep "<words>" --speaker "<name>""#,
        },
        Example {
            about: "Read a meeting",
            command: "grans show <id-or-title>",
        },
        Example {
            about: "Read a meeting's whole transcript",
            command: "grans show <id-or-title> --transcript",
        },
        Example {
            about: "See meetings with someone",
            command: r#"grans with "<name>""#,
        },
        Example {
            about: "See meetings from a period",
            command: "grans list --date last-week",
        },
        Example {
            about: "Get new meetings from Granola",
            command: "grans sync --all",
        },
    ],
    workflows: &[
        Workflow {
            name: "Set up on a new machine",
            steps: &[
                Example {
                    command: "grans auth login",
                    about: "Sign in to Granola",
                },
                Example {
                    command: "grans sync --all",
                    about: "Download everything and make it searchable",
                },
            ],
            lines: &[
                "On Windows and Linux grans can reuse the Granola desktop app's sign-in,",
                "so the login is optional there. The first sync makes two requests per",
                "meeting, 1.5 seconds apart.",
            ],
        },
        Workflow {
            name: "Keep data current",
            steps: &[Example {
                command: "grans sync --all",
                about: "New meetings, transcripts, AI notes, then embeddings",
            }],
            lines: &[
                "Plain `grans sync` is quicker but skips transcripts, AI notes, and",
                "embeddings. AI notes are the summaries Granola generates for a meeting.",
                "Embeddings are what `grans search` uses to match by meaning.",
            ],
        },
        Workflow {
            name: "Research a topic across meetings",
            steps: &[
                Example {
                    command: r#"grans search "<topic>""#,
                    about: "Find the most relevant meetings",
                },
                Example {
                    command: r#"grans grep "<term>" --limit 0"#,
                    about: "List every meeting that mentions a term",
                },
                Example {
                    command: "grans show <id> --transcript",
                    about: "Read the ones that matter",
                },
            ],
            lines: &[
                "AI notes are machine-written: use them to find meetings, and confirm",
                "what matters against the transcript.",
            ],
        },
        Workflow {
            name: "Script against the data",
            steps: &[Example {
                command: r#"grans grep "<words>" --limit 0 --json"#,
                about: "Every match, as JSON",
            }],
            lines: &[
                "Add --json to any command. In loops use grep, not search: grep loads no",
                "models, and search loads them on every run.",
            ],
        },
    ],
    closing: &[
        "Run `grans <command> --help` for that command's flags, examples, and notes.",
        "`-h` prints a shorter summary without them.",
    ],
};
