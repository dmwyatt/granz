//! Help for statistics about meetings in aggregate.

use super::{CommandHelp, DATES, Example, Section};

/// What every statistic shares about how it reads the transcript.
const MEASURES: Section = Section {
    title: "How things are measured",
    lines: &[
        "Who said how much is counted in characters of transcript, because",
        "Granola's end times are not reliable enough to time speech. A meeting's",
        "spoken time runs from its first word to its last, leaving out any",
        "silence longer than 30 minutes (a note reopened later for another talk).",
        "",
        "\"You\" is your microphone; everyone else shares the other audio",
        "channel. Granola has named the speakers on that channel only for",
        "meetings recorded since it started detecting them, so anything about",
        "named speakers covers only those meetings.",
        "",
        "Invitees are who the calendar invited, not who came.",
    ],
};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "stats",
        examples: &[
            Example {
                command: "grans stats",
                about: "How many meetings, how much time, and how much of it you talked",
            },
            Example {
                command: "grans stats --date last-month",
                about: "The same for last month",
            },
            Example {
                command: "grans stats meetings",
                about: "The numbers behind it, one meeting per line",
            },
        ],
        sections: &[MEASURES, DATES],
    },
    CommandHelp {
        path: "stats meetings",
        examples: &[
            Example {
                command: "grans stats meetings --date this-week",
                about: "This week's meetings: length, your share, invitees, speakers",
            },
            Example {
                command: "grans stats meetings --json",
                about: "Everything known about each meeting, for your own analysis",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "The JSON has a row per meeting: its calendar slot and recurring",
                    "series, who organized it, each invitee with their response, and a",
                    "line per voice (you, each named speaker, everyone else unnamed)",
                    "with characters, utterances, questions, and first and last word.",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
];
