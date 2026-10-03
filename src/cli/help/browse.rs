//! Help for looking through what Granola keeps besides meetings.

use super::{CommandHelp, DATES, Example, Section};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "browse",
        examples: &[
            Example {
                command: "grans browse people list",
                about: "The people Granola has on record",
            },
            Example {
                command: r#"grans browse people show "jane""#,
                about: "One person's details",
            },
            Example {
                command: "grans browse calendars events --date this-week",
                about: "This week's calendar events",
            },
            Example {
                command: "grans browse templates list",
                about: "The templates AI notes are generated from",
            },
            Example {
                command: "grans browse recipes list",
                about: "Granola's recipes (saved prompts)",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "These look through what Granola keeps besides the meetings themselves.",
                "For meetings, use `grans list`, `grans search`, or `grans with`.",
            ],
        }],
    },
    CommandHelp {
        path: "browse people",
        examples: &[
            Example {
                command: "grans browse people list",
                about: "Everyone, by name",
            },
            Example {
                command: r#"grans browse people list --company "Acme""#,
                about: "Only people at one company",
            },
            Example {
                command: r#"grans browse people show "jane""#,
                about: "Everyone matching a name or email address",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[r#"For the meetings someone was invited to, use `grans with "<name>"`."#],
        }],
    },
    CommandHelp {
        path: "browse people list",
        examples: &[
            Example {
                command: "grans browse people list",
                about: "Everyone, by name",
            },
            Example {
                command: r#"grans browse people list --company "Acme""#,
                about: "Only people at one company",
            },
            Example {
                command: "grans browse people list --json",
                about: "As JSON",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse people show",
        examples: &[
            Example {
                command: r#"grans browse people show "jane""#,
                about: "Everyone whose name or email address contains jane",
            },
            Example {
                command: r#"grans browse people show "jane@example.com" --json"#,
                about: "One person, as JSON",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Shows each match with their job title and company when Granola has",
                "them. Nobody matching is an error.",
            ],
        }],
    },
    CommandHelp {
        path: "browse calendars",
        examples: &[
            Example {
                command: "grans browse calendars list",
                about: "The calendars Granola reads",
            },
            Example {
                command: "grans browse calendars events --date this-week",
                about: "This week's calendar events",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse calendars list",
        examples: &[
            Example {
                command: "grans browse calendars list",
                about: "Each calendar, with the start of its ID",
            },
            Example {
                command: "grans browse calendars list --json",
                about: "As JSON, with whole IDs",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse calendars events",
        examples: &[
            Example {
                command: "grans browse calendars events",
                about: "Every calendar event grans has synced",
            },
            Example {
                command: "grans browse calendars events --date this-week",
                about: "Only this week's",
            },
            Example {
                command: "grans browse calendars events --from 2026-01-01 --to 2026-01-31",
                about: "Between two dates",
            },
            Example {
                command: r#"grans browse calendars events --calendar "jane@example.com""#,
                about: "Only one calendar, by part of its ID",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "These are calendar entries, not meetings: for what was recorded, use",
                    "`grans list`. `grans browse calendars list --json` gives each",
                    "calendar's ID, and --calendar matches any part of one.",
                ],
            },
            DATES,
        ],
    },
    CommandHelp {
        path: "browse templates",
        examples: &[
            Example {
                command: "grans browse templates list",
                about: "Every template",
            },
            Example {
                command: r#"grans browse templates show "Stand-Up""#,
                about: "One template's description and sections",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "A template is the outline Granola fills in when it generates AI notes",
                "for a meeting.",
            ],
        }],
    },
    CommandHelp {
        path: "browse templates list",
        examples: &[
            Example {
                command: "grans browse templates list",
                about: "Every template",
            },
            Example {
                command: r#"grans browse templates list --category "Team""#,
                about: "Only one category",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse templates show",
        examples: &[
            Example {
                command: r#"grans browse templates show "Stand-Up""#,
                about: "A template by part of its title, or by ID",
            },
            Example {
                command: r#"grans browse templates show "Stand-Up" --json"#,
                about: "As JSON",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse recipes",
        examples: &[
            Example {
                command: "grans browse recipes list",
                about: "Every recipe",
            },
            Example {
                command: r#"grans browse recipes show "meeting-summary""#,
                about: "One recipe's description and instructions",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["A recipe is a saved prompt in Granola: a name with instructions."],
        }],
    },
    CommandHelp {
        path: "browse recipes list",
        examples: &[
            Example {
                command: "grans browse recipes list",
                about: "Every recipe",
            },
            Example {
                command: "grans browse recipes list --visibility public",
                about: "Only public ones (also: shared, user, unlisted)",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "browse recipes show",
        examples: &[
            Example {
                command: r#"grans browse recipes show "meeting-summary""#,
                about: "A recipe by part of its name, or by ID",
            },
            Example {
                command: r#"grans browse recipes show "meeting-summary" --json"#,
                about: "As JSON",
            },
        ],
        sections: &[],
    },
];
