//! Help for the commands that find and read meetings.

use super::{CommandHelp, DATES, Example, Section};

/// What AI notes are and how far to trust them, shown wherever they surface.
const AI_NOTES: Section = Section {
    title: "AI notes",
    lines: &[
        "AI notes are the summaries Granola generates for a meeting. Granola calls",
        "them panels, which is why search and grep say `--in panels`. grans cannot",
        "see the prompt, model, or settings that produced them, so treat them as",
        "leads: use them to find a meeting, then confirm anything that matters",
        "against the transcript with `grans show <id> --transcript`.",
    ],
};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "search",
        examples: &[
            Example {
                command: r#"grans search "pricing decision""#,
                about: "The best few meetings about a topic",
            },
            Example {
                command: r#"grans search "pricing" --date last-month"#,
                about: "Only meetings from last month",
            },
            Example {
                command: r#"grans search "pricing" --in transcripts"#,
                about: "Only what was said aloud",
            },
            Example {
                command: r#"grans search "pricing" --meeting "Weekly""#,
                about: "Only meetings whose title contains Weekly",
            },
            Example {
                command: r#"grans search "pricing" --matches 3 --context 2"#,
                about: "Three excerpts per meeting, with the lines around each",
            },
            Example {
                command: r#"grans search "pricing" --min-score 0.5"#,
                about: "Drop weakly related meetings",
            },
            Example {
                command: r#"grans search "pricing" --fast"#,
                about: "Skip the rerank stage (no relevance scores)",
            },
            Example {
                command: r#"grans search "pricing" --limit 25 --json"#,
                about: "Up to 25 meetings, as JSON",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Results are the best few meetings for the query, not every match. When",
                    "any meeting contains the query's words, a footer gives that count and the",
                    "`grans grep` command that lists them all.",
                    "",
                    "Search reads embeddings and never builds them. It warns on stderr when",
                    "meetings have synced since the last `grans embed` (recent ones may be",
                    "missing from the results) and when there are no usable embeddings (the",
                    "results are then keyword matches only). Run `grans embed` to fix either.",
                    "",
                    "Every run loads the embedding model, and the reranker too unless --fast",
                    "is given, so search is much slower than `grans grep`, which loads no",
                    "models. For many lookups in a loop, use grep.",
                    "",
                    "There is no --speaker here. To find what one person said, use",
                    r#"`grans grep "<words>" --speaker "<name>"`."#,
                ],
            },
            Section {
                title: "JSON output",
                lines: &[
                    "One object: query, keyword_total, limit, returned, meetings.",
                    "",
                    "keyword_total counts the meetings containing the query's words, the",
                    "number `grans grep` reports as total_meetings. There is no total for",
                    "the ranked list.",
                    "",
                    "Each meeting has id, title, created_at, total_matches, matches, and",
                    "signals: which of keyword, semantic, and title found it. It also has",
                    "score, a relevance from 0 to 1, unless --fast skipped the rerank stage.",
                    "",
                    "Each match has source (transcript, panel, or notes), snippet, and",
                    "highlights (character ranges within snippet). Transcript matches add",
                    "timestamp, speaker (me or other), and speaker_name when Granola named",
                    "the speaker. Panel matches add section. --context adds context_before",
                    "and context_after.",
                ],
            },
            DATES,
            AI_NOTES,
        ],
    },
    CommandHelp {
        path: "grep",
        examples: &[
            Example {
                command: r#"grans grep "pricing""#,
                about: "Every meeting that mentions pricing",
            },
            Example {
                command: r#"grans grep "pricing review""#,
                about: "Meetings containing both words, in any order",
            },
            Example {
                command: r#"grans grep '"pricing review"'"#,
                about: "Meetings containing that exact phrase",
            },
            Example {
                command: r#"grans grep "pricing" --limit 0"#,
                about: "Show every match instead of the first 10",
            },
            Example {
                command: r#"grans grep "pricing" --speaker "Jane""#,
                about: "Only where Jane said it",
            },
            Example {
                command: r#"grans grep "pricing" --speaker me"#,
                about: "Only where you said it",
            },
            Example {
                command: r#"grans grep "pricing" --in titles"#,
                about: "Only meeting titles",
            },
            Example {
                command: r#"grans grep "pricing" --from 2w --context 2"#,
                about: "The last two weeks, with the lines around each match",
            },
            Example {
                command: r#"grans grep "pricing" --limit 0 --matches 0"#,
                about: "A compact list: meetings without excerpts",
            },
            Example {
                command: r#"grans grep "pricing" --limit 0 --json"#,
                about: "Every match, as JSON",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "The count grep reports is complete: every synced meeting that contains",
                    "the words. --limit only trims how many are shown, best matches first.",
                    "",
                    "All the words must appear, in any order, as whole words. Put a phrase in",
                    "double quotes inside the query to require it exactly.",
                    "",
                    "--speaker counts only transcript lines by that speaker, so --in must",
                    r#"include transcripts. "me" is your own microphone and "other" is everyone"#,
                    "else; both work on every meeting. A name matches the speaker Granola",
                    "identified, which it does only for other people and only in meetings",
                    "recorded since 2026-07-21. Part of a name is enough. A name that matches",
                    "nobody is an error listing the names that exist, so a typo never looks",
                    "like an empty result.",
                    "",
                    "grep loads no models and never prompts, so it suits scripts and loops.",
                    "To find meetings by meaning rather than exact words, use `grans search`.",
                ],
            },
            Section {
                title: "JSON output",
                lines: &[
                    "One object: query, total_meetings, limit, returned, meetings.",
                    "",
                    "total_meetings counts every meeting containing the words; --limit only",
                    "shortens meetings. Each meeting is shaped as in `grans search --json`",
                    "output, without score.",
                ],
            },
            DATES,
            AI_NOTES,
        ],
    },
    CommandHelp {
        path: "list",
        examples: &[
            Example {
                command: "grans list",
                about: "Every meeting, newest first",
            },
            Example {
                command: "grans list --date this-week",
                about: "This week's meetings",
            },
            Example {
                command: "grans list --from 2w",
                about: "The last two weeks",
            },
            Example {
                command: "grans list --from 2026-01-01 --to 2026-01-31",
                about: "Between two dates",
            },
            Example {
                command: r#"grans list --person "jane""#,
                about: "Meetings Jane was invited to",
            },
            Example {
                command: "grans list --include-deleted",
                about: "Also meetings deleted in Granola",
            },
            Example {
                command: "grans list --json",
                about: "As JSON, with each meeting's notes",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Each row starts with the meeting's ID. Pass it to `grans show`.",
                    "",
                    "--person matches part of an invitee's name or email address, as",
                    "`grans with` does.",
                ],
            },
            DATES,
            Section {
                title: "JSON output",
                lines: &[
                    "An array of meetings, each with id, title, created_at, notes_plain,",
                    "notes_markdown, people, and the other fields Granola keeps for it.",
                    "Transcripts are not included; get one with",
                    "`grans show <id> --transcript --json`.",
                ],
            },
        ],
    },
    CommandHelp {
        path: "show",
        examples: &[
            Example {
                command: r#"grans show "Weekly Sync""#,
                about: "Details, the transcript's first lines, and AI notes",
            },
            Example {
                command: "grans show 3219f4e3",
                about: "The same, by the ID that list, search, and grep print",
            },
            Example {
                command: "grans show 3219f4e3 --transcript",
                about: "The whole transcript",
            },
            Example {
                command: "grans show 3219f4e3 --notes",
                about: "Only the notes you wrote",
            },
            Example {
                command: "grans show 3219f4e3 --notes --transcript",
                about: "Your notes, then the transcript",
            },
            Example {
                command: r#"grans show 3219f4e3 --transcript --speaker "Jane""#,
                about: "Only what Jane said",
            },
            Example {
                command: "grans show 3219f4e3 --transcript --json",
                about: "The transcript as JSON",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "A title fragment that matches several meetings shows only one of them.",
                    "Use the ID to be sure which. Meetings deleted in Granola cannot be",
                    "shown.",
                    "",
                    "Without --transcript, only the first 10 transcript lines are shown.",
                    "",
                    r#"--speaker takes "me", "other", or a name; `grans grep --help` says how"#,
                    "each is matched.",
                ],
            },
            Section {
                title: "JSON output",
                lines: &[
                    "The meeting's fields as in `grans list --json` output, plus panels (the",
                    "AI notes) when it has any. The transcript is not included.",
                    "",
                    "With --transcript or --notes the object holds only what was asked for:",
                    "transcript, an array of lines with text, source, start_timestamp,",
                    "end_timestamp, and detected_speaker_name; and notes_plain with",
                    "notes_markdown. source is microphone for you and system for everyone",
                    "else.",
                ],
            },
            AI_NOTES,
        ],
    },
    CommandHelp {
        path: "with",
        examples: &[
            Example {
                command: r#"grans with "jane""#,
                about: "Meetings Jane was invited to, newest first",
            },
            Example {
                command: r#"grans with "jane@example.com""#,
                about: "The same, by email address",
            },
            Example {
                command: r#"grans with "jane" --date last-month"#,
                about: "Only last month's",
            },
            Example {
                command: r#"grans with "jane" --json"#,
                about: "As JSON, shaped as `grans list --json` output",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Finds meetings whose calendar invitation lists the person, matching part",
                    "of a name or email address. An invitation is not attendance: someone",
                    "listed may not have come. The transcript shows who spoke.",
                    "",
                    "grans stores a name for few invitees, so part of an email address finds",
                    "meetings that a name misses.",
                    "",
                    "`grans browse people list` shows the people grans knows about.",
                ],
            },
            DATES,
        ],
    },
    CommandHelp {
        path: "recent",
        examples: &[
            Example {
                command: "grans recent",
                about: "This week's meetings",
            },
            Example {
                command: "grans recent --json",
                about: "As JSON",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Short for `grans list --date this-week`."],
        }],
    },
    CommandHelp {
        path: "today",
        examples: &[
            Example {
                command: "grans today",
                about: "Today's meetings",
            },
            Example {
                command: "grans today --json",
                about: "As JSON",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["Short for `grans list --date today`."],
        }],
    },
    CommandHelp {
        path: "info",
        examples: &[
            Example {
                command: "grans info",
                about: "What the local database holds",
            },
            Example {
                command: "grans info --json",
                about: "The same numbers as JSON",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Counts what is stored (meetings with and without a transcript, people,",
                "calendar events, AI notes, embedded chunks, and more), and gives the",
                "date range covered and the database's path and size.",
                "",
                "`grans embed status` shows what still needs embedding, and",
                "`grans admin db info` shows when each kind of data last synced.",
            ],
        }],
    },
];
