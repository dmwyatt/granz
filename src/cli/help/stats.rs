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
    CommandHelp {
        path: "stats speakers",
        examples: &[
            Example {
                command: "grans stats speakers",
                about: "Who talks most, and who was invited but never heard",
            },
            Example {
                command: "grans stats speakers --date last-month --json",
                about: "Last month's speakers, series and evenness as JSON",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Covers only meetings where Granola named at least one speaker. A",
                    "speaker is a name as Granola heard it, compared without case or extra",
                    "spaces. It is matched to the invitee with the same name, or else to",
                    "the one invitee of that meeting sharing its first name. Two people on",
                    "one microphone come out as one speaker.",
                    "",
                    "For each speaker: meetings they were heard in and their share of the",
                    "talk there; meetings they were invited to, of which never heard; and",
                    "meetings they spoke in without an invitation. Then how often you",
                    "spoke right after them and they after you, who talks most in each",
                    "series, and how evenly talk is spread among you and the named",
                    "speakers (1 is perfectly even).",
                ],
            },
            MEASURES,
            DATES,
        ],
    },
    CommandHelp {
        path: "stats words",
        examples: &[
            Example {
                command: "grans stats words",
                about: "Your verbal tics, your signature words, and terms new to your meetings",
            },
            Example {
                command: r#"grans stats words --term "roadmap" --term "hiring plan""#,
                about: "How often those came up, month by month, and when first",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Tics are phrases such as \"you know\" and \"kind of\" in your own",
                    "speech, per thousand of your words; Granola already removes most",
                    "single-word fillers. Signature words are the ones you say far more",
                    "than everyone else, scored by a log-odds ratio. New terms are words",
                    "first heard after the first tenth of the period's words, most mentioned",
                    "first. A --term can be a phrase; it matches whole words, ignoring case.",
                    "",
                    "Then people whose full name was said in meetings they were not",
                    "invited to (or declined). Full names only: first names alone are too",
                    "often ordinary words or someone else.",
                    "",
                    "Common words are left out of the vocabulary lists. This reads every",
                    "utterance's text, so it takes longer than the other statistics.",
                ],
            },
            DATES,
        ],
    },
    CommandHelp {
        path: "stats topics",
        examples: &[
            Example {
                command: "grans stats topics",
                about: "Topic groups, repetitive series, overlapping series, and outliers",
            },
            Example {
                command: "grans stats topics --json",
                about: "The same, with every meeting's place on a map of topics",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Uses the embeddings `grans embed` builds; meetings it has not reached",
                    "are left out. A meeting is the average of its transcript's embedded",
                    "windows, and two meetings are as alike as the cosine between them.",
                    "",
                    "Topic groups come from k-means into 8 groups, named by words their",
                    "titles use more than other titles do. A recurring meeting that",
                    "repeats itself has consecutive meetings that are very alike. Series",
                    "with nearly the same content are compared by their average, over",
                    "series with at least 3 embedded meetings and different titles (a",
                    "series split by a calendar edit keeps its title). Outliers are the",
                    "meetings whose most similar other meeting is least similar.",
                    "",
                    "Similarities rank meetings against each other; they are not a",
                    "percentage, and verbatim repeats do not spike them.",
                ],
            },
            DATES,
        ],
    },
    CommandHelp {
        path: "stats notes",
        examples: &[
            Example {
                command: "grans stats notes",
                about: "Action items per series, the ones naming you, and your typed notes",
            },
            Example {
                command: "grans stats notes --date this-month --limit 0",
                about: "Every action item naming you this month",
            },
        ],
        sections: &[
            Section {
                title: "Notes",
                lines: &[
                    "Action items are the bullets under \"Next steps\" or \"Action items\"",
                    "headings in Granola's AI notes. One names you when your first name, as",
                    "your notes give it, is in the item or the name it is listed under.",
                    "",
                    "Per month: meetings, how many you typed notes in, action items, and",
                    "the summary templates that wrote the AI notes. Template names date",
                    "Granola's changes to how it summarizes.",
                ],
            },
            DATES,
        ],
    },
];
