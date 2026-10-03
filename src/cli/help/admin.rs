//! Help for maintaining the local database and measuring search.

use super::{CommandHelp, Example, Section};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "admin",
        examples: &[
            Example {
                command: "grans admin db info",
                about: "Where the database is and whether it is healthy",
            },
            Example {
                command: "grans admin db rebuild-fts",
                about: "Repair the word index that `grans grep` relies on",
            },
            Example {
                command: "grans admin token",
                about: "Print the Granola token grans would use",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "admin db",
        examples: &[
            Example {
                command: "grans admin db info",
                about: "Path, size, last sync times, accounts, and index health",
            },
            Example {
                command: "grans admin db list",
                about: "Every database file in grans's data directory",
            },
            Example {
                command: "grans admin db rebuild-fts",
                about: "Repair the word indexes",
            },
            Example {
                command: "grans admin db clear",
                about: "Delete the database, without asking",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "info, rebuild-fts, and clear act on the default database, or on the one",
                "named by --db. `grans info` shows what the database contains.",
            ],
        }],
    },
    CommandHelp {
        path: "admin db clear",
        examples: &[
            Example {
                command: "grans admin db clear",
                about: "Delete the database",
            },
            Example {
                command: "grans admin db clear --all",
                about: "Delete every database file in the data directory",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Deletes without asking. `grans sync --all` downloads the data again,",
                "which takes two requests per meeting, 1.5 seconds apart.",
            ],
        }],
    },
    CommandHelp {
        path: "admin db info",
        examples: &[
            Example {
                command: "grans admin db info",
                about: "The default database",
            },
            Example {
                command: "grans --db ./other.db admin db info",
                about: "Another database file",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Ends with one line per word index saying whether it still agrees with",
                "the data it indexes. An index that has drifted makes `grans grep` and",
                "`grans search` return too few results while everything else looks fine;",
                "`grans admin db rebuild-fts` repairs it.",
                "",
                "Also lists each Granola account the database has synced from.",
            ],
        }],
    },
    CommandHelp {
        path: "admin db list",
        examples: &[Example {
            command: "grans admin db list",
            about: "Every database file in grans's data directory, with its size",
        }],
        sections: &[],
    },
    CommandHelp {
        path: "admin db rebuild-fts",
        examples: &[Example {
            command: "grans admin db rebuild-fts",
            about: "Rebuild every word index from the data it indexes",
        }],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Run this when `grans admin db info` reports an index that has drifted.",
                "Nothing is lost and nothing needs syncing again.",
            ],
        }],
    },
    CommandHelp {
        path: "admin token",
        examples: &[
            Example {
                command: "grans admin token",
                about: "Print the Granola token grans would use",
            },
            Example {
                command: "grans admin token --clipboard",
                about: "Copy it to the clipboard instead of printing it",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "The token grants access to your Granola account, so keep it out of",
                "logs and shared terminals. `grans auth --help` gives the order grans",
                "looks for one in.",
            ],
        }],
    },
    CommandHelp {
        path: "benchmark",
        examples: &[
            Example {
                command: "grans benchmark quality --file golden.json",
                about: "How often search finds the meetings it should",
            },
            Example {
                command: "grans benchmark semantic-search",
                about: "How fast the vector search runs",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "benchmark semantic-search",
        examples: &[
            Example {
                command: "grans benchmark semantic-search",
                about: "Time 100 queries against the stored embeddings",
            },
            Example {
                command: "grans benchmark semantic-search --queries 500 --warmup 20",
                about: "More queries, after a longer warm-up",
            },
            Example {
                command: "grans benchmark semantic-search --synthetic --vectors 50000",
                about: "Against generated vectors instead of real data",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "benchmark quality",
        examples: &[
            Example {
                command: "grans benchmark quality --file golden.json",
                about: "Score semantic search, the default mode",
            },
            Example {
                command: "grans benchmark quality --file golden.json --mode rerank-jina",
                about: "Score what `grans search` runs",
            },
            Example {
                command: "grans benchmark quality --file golden.json --compare fts,semantic",
                about: "Compare modes query by query",
            },
            Example {
                command: "grans benchmark quality --file golden.json --k 5 --detail",
                about: "Score the top 5, showing each query's results",
            },
            Example {
                command: r#"grans benchmark quality --file golden.json --record --note "baseline""#,
                about: "Keep the run in the ledger",
            },
        ],
        sections: &[
            Section {
                title: "Golden set file",
                lines: &[
                    "A JSON object with a queries array. Each entry has query (the text to",
                    "search for) and the meetings it should find: relevant_meeting_ids",
                    "(meeting IDs) or, when that is absent, relevant_meetings (exact",
                    "titles). IDs are safer, since recurring meetings share a title. An",
                    "optional query_type groups the report by kind of query.",
                ],
            },
            Section {
                title: "Notes",
                lines: &[
                    "Reports hit-rate@k (queries with an expected meeting in the top k),",
                    "recall@k (the share of expected meetings found in the top k), MRR@k",
                    "(1/rank of the first expected meeting), and latency per query.",
                    "",
                    "Use --db to score a different database without touching your own.",
                ],
            },
        ],
    },
];
