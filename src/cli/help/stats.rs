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
            Example {
                command: "grans stats --html meetings.html",
                about: "Charts of it all, as a page to open in a browser",
            },
        ],
        sections: &[
            Section {
                title: "The page",
                lines: &[
                    "--html writes one file with the data and charts inside it. Open it in",
                    "a browser; it fetches nothing, so it works offline and your meetings",
                    "stay on this machine. Its filters narrow every chart at once. The date",
                    "filters choose which meetings go into the file.",
                ],
            },
            MEASURES,
            DATES,
        ],
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
    CommandHelp {
        path: "stats time",
        examples: &[
            Example {
                command: "grans stats time",
                about: "Hours of meetings per month, and how they break up the day",
            },
            Example {
                command: "grans stats time --by week --date last-month",
                about: "The same per week, for last month",
            },
            Example {
                command: "grans stats time --json",
                about: "Every day's numbers too, for your own analysis",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "For each period: meetings, hours, and days with meetings; the typical",
                    "first and last spoken word of a meeting day; the typical longest gap",
                    "between meetings from 09:00 to 17:00; and the most meetings in a row",
                    "(no more than 10 minutes apart) on one day. Then meeting sizes, by",
                    "invitees who did not decline, and hours by weekday and hour of day.",
                    "",
                    "Days and hours are in your time zone, or UTC with --utc.",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
    CommandHelp {
        path: "stats series",
        examples: &[
            Example {
                command: "grans stats series",
                about: "Your recurring meetings, most hours first",
            },
            Example {
                command: "grans stats series --limit 0 --json",
                about: "Every series, for your own analysis",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "A series is the calendar's recurring event; its title is the latest",
                    "meeting's. Editing a recurring meeting from some date on can give the",
                    "rest a new series, so one meeting can appear as several.",
                    "",
                    "For each series: meetings, hours, first and last meeting, the",
                    "median days between meetings, invitees who did not decline at the",
                    "first and the latest meeting, the median minutes the talk started",
                    "after the slot began and ran past its end, and your share of the talk.",
                    "",
                    "Punctuality uses a meeting's first stretch of talk, so a note reopened",
                    "later for another conversation does not count as running over.",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
    CommandHelp {
        path: "stats people",
        examples: &[
            Example {
                command: "grans stats people",
                about: "Who you meet with most, and your one-on-ones with each",
            },
            Example {
                command: "grans stats people --date this-month --limit 5",
                about: "This month's five most frequent invitees",
            },
            Example {
                command: "grans stats people --json",
                about: "Everything, including who is invited together",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "A person is an invitee's email address. Meetings with someone are",
                    "meetings they were invited to and did not decline; an invitation does",
                    "not say whether they came. A one-on-one is a meeting with exactly one",
                    "such invitee, so everyone else's audio is that person even in meetings",
                    "from before Granola named speakers.",
                    "",
                    "Then: who organized your meetings, invitees by email domain and how",
                    "many meetings included someone outside yours, how you answered the",
                    "invitations of meetings you recorded, and who is most often invited",
                    "together, counting meetings of up to 10 invitees.",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
    CommandHelp {
        path: "stats talk",
        examples: &[
            Example {
                command: "grans stats talk",
                about: "How much of the talking you do, by meeting size and month",
            },
            Example {
                command: "grans stats talk --date last-month --json",
                about: "Last month's numbers, with every silent meeting listed",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Your share is your characters of transcript over everyone's. Back and",
                    "forth is how often the talk passes between you and everyone else, per",
                    "hour: high in a discussion, low when someone presents. Questions are",
                    "utterances ending in a question mark, as a share of each side's.",
                    "",
                    "The tenths split each meeting's talk into ten equal parts, first to",
                    "last, to show whether you speak up early or late. Then meetings where",
                    "you never spoke, and meetings with only your microphone, which are",
                    "often in person with everyone on that one microphone.",
                    "",
                    "This uses the audio channels, not speaker names, so it covers every",
                    "meeting with a transcript.",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
];
