//! Help for getting data from Granola and making it searchable.

use super::{CommandHelp, Example, Section};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "sync",
        examples: &[
            Example {
                command: "grans sync --all",
                about: "Everything: meetings, transcripts, AI notes, then embeddings",
            },
            Example {
                command: "grans sync --all --retry",
                about: "Also retry fetches that failed before",
            },
            Example {
                command: "grans sync --all --dry-run",
                about: "Show what would be fetched, and change nothing",
            },
            Example {
                command: "grans sync",
                about: "Only the quick part: meeting list, people, calendars",
            },
            Example {
                command: "grans sync transcripts --since 2w --embed",
                about: "Recent transcripts, then embeddings",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Plain `grans sync` skips transcripts, AI notes, and embeddings. It",
                "fetches the meeting list with your own notes, people, calendar events,",
                "and Granola's templates and recipes.",
                "",
                "Transcripts and AI notes take one request per meeting, 1.5 seconds",
                "apart, so a first `grans sync --all` is slow. Later runs fetch only",
                "what is missing. A meeting whose fetch failed is skipped from then on;",
                "--retry tries those again.",
                "",
                "Only `grans sync --all`, `grans sync transcripts --embed`, and",
                "`grans embed` build embeddings. Content that is not embedded yet can be",
                "missing from `grans search` results.",
                "",
                "The subcommands each sync one kind of data. Two use Granola's names:",
                "documents are meetings, and panels are AI notes (the summaries Granola",
                "generates for a meeting).",
                "",
                "Syncing needs a Granola sign-in; see `grans auth login`. Progress is",
                "written to stderr.",
            ],
        }],
    },
    CommandHelp {
        path: "sync documents",
        examples: &[
            Example {
                command: "grans sync documents",
                about: "The meeting list: titles, invitees, and your notes",
            },
            Example {
                command: "grans sync documents --dry-run",
                about: "Show what would change",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Granola calls a meeting's record a document. This fetches no transcripts",
                "or AI notes; `grans sync --all` does.",
            ],
        }],
    },
    CommandHelp {
        path: "sync transcripts",
        examples: &[
            Example {
                command: "grans sync transcripts",
                about: "Transcripts for meetings that have none yet",
            },
            Example {
                command: "grans sync transcripts --since 2w --embed",
                about: "Only recent meetings, then build embeddings",
            },
            Example {
                command: "grans sync transcripts --retry",
                about: "Also meetings whose fetch failed before",
            },
            Example {
                command: "grans sync transcripts --limit 50",
                about: "Stop after 50 meetings",
            },
            Example {
                command: "grans sync transcripts 504fe9f6",
                about: "Fetch one meeting's transcript again, by ID",
            },
            Example {
                command: "grans sync transcripts --dry-run",
                about: "Show which meetings would be fetched",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "One request per meeting, --delay-ms apart (1500 by default). A meeting",
                "whose fetch failed or returned no transcript is skipped on later runs",
                "until --retry.",
                "",
                "Given an ID (whole, or a prefix that matches one meeting), the",
                "transcript is fetched even if one is stored, and replaces it.",
                "",
                "--since takes a date (2026-01-15) or an age such as 3d, 2w, or 1m. A",
                "value grans cannot read is ignored, so every meeting is considered.",
                "",
                "--embed builds embeddings afterwards without asking, as `grans embed -y`",
                "does.",
            ],
        }],
    },
    CommandHelp {
        path: "sync panels",
        examples: &[
            Example {
                command: "grans sync panels",
                about: "AI notes for meetings that have none yet",
            },
            Example {
                command: "grans sync panels --since 2w",
                about: "Only recent meetings",
            },
            Example {
                command: "grans sync panels --retry",
                about: "Also meetings whose fetch failed before",
            },
            Example {
                command: "grans sync panels --dry-run",
                about: "Show which meetings would be fetched",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Panels are Granola's name for the AI notes it generates for a meeting.",
                "One request per meeting, --delay-ms apart (1500 by default). A meeting",
                "whose fetch failed or returned no panels is skipped on later runs until",
                "--retry.",
                "",
                "--since takes a date (2026-01-15) or an age such as 3d, 2w, or 1m. A",
                "value grans cannot read is ignored, so every meeting is considered.",
                "",
                "This builds no embeddings. Run `grans embed` afterwards so that",
                "`grans search` can match the new notes by meaning.",
            ],
        }],
    },
    CommandHelp {
        path: "sync people",
        examples: &[
            Example {
                command: "grans sync people",
                about: "The people Granola has on record",
            },
            Example {
                command: "grans sync people --dry-run",
                about: "Show what would change",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Plain `grans sync` includes this."],
        }],
    },
    CommandHelp {
        path: "sync calendars",
        examples: &[
            Example {
                command: "grans sync calendars",
                about: "Your calendars and their events",
            },
            Example {
                command: "grans sync calendars --dry-run",
                about: "Show what would change",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Plain `grans sync` includes this."],
        }],
    },
    CommandHelp {
        path: "sync templates",
        examples: &[
            Example {
                command: "grans sync templates",
                about: "The templates Granola generates AI notes from",
            },
            Example {
                command: "grans sync templates --dry-run",
                about: "Show what would change",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Plain `grans sync` includes this."],
        }],
    },
    CommandHelp {
        path: "sync recipes",
        examples: &[
            Example {
                command: "grans sync recipes",
                about: "Granola's recipes (saved prompts)",
            },
            Example {
                command: "grans sync recipes --dry-run",
                about: "Show what would change",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Plain `grans sync` includes this."],
        }],
    },
    CommandHelp {
        path: "embed",
        examples: &[
            Example {
                command: "grans embed",
                about: "Embed new and changed content, after asking",
            },
            Example {
                command: "grans embed -y",
                about: "The same without asking; use this in scripts",
            },
            Example {
                command: "grans embed status",
                about: "How much is embedded and how much is waiting",
            },
            Example {
                command: "grans embed --batch-size 32",
                about: "Bigger batches: more memory, possibly faster on a GPU",
            },
            Example {
                command: "grans embed clear -y",
                about: "Delete every embedding",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Embeddings are what `grans search` uses to match by meaning. Three",
                "commands build them: `grans embed`, `grans sync --all`, and",
                "`grans sync transcripts --embed`. Nothing else does; `grans search`",
                "only reads them.",
                "",
                "Embedding is incremental: content that is already embedded is skipped.",
                "When a new version of grans changes the embedding model or how text is",
                "split up, the next run rebuilds everything once.",
                "",
                "`grans embed` asks before it starts unless given -y or --json, so pass",
                "-y when nobody is there to answer. The two sync commands never ask.",
                "",
                "The first run downloads the embedding model.",
            ],
        }],
    },
    CommandHelp {
        path: "embed status",
        examples: &[
            Example {
                command: "grans embed status",
                about: "Embedded and pending counts, by kind of content",
            },
            Example {
                command: "grans embed status --json",
                about: "The same as JSON",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Pending is content that has synced but is not embedded yet;",
                "`grans embed` embeds it. This only reads: it changes nothing and loads",
                "no model.",
            ],
        }],
    },
    CommandHelp {
        path: "embed clear",
        examples: &[
            Example {
                command: "grans embed clear",
                about: "Delete every embedding, after asking",
            },
            Example {
                command: "grans embed clear -y",
                about: "The same without asking",
            },
            Example {
                command: "grans embed clear --count 100",
                about: "Delete only the 100 most recent",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Meant for development and testing. Asks first unless given -y or",
                "--json. With every embedding gone, `grans search` returns keyword",
                "matches only until `grans embed` rebuilds them.",
            ],
        }],
    },
];
