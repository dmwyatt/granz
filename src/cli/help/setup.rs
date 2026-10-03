//! Help for signing in, sharing the database between machines, updating
//! grans, and teaching an AI agent to use it.

use super::{CommandHelp, Example, Section};

pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        path: "auth",
        examples: &[
            Example {
                command: "grans auth login",
                about: "Sign in to Granola in a browser",
            },
            Example {
                command: "grans auth status",
                about: "Which account grans is signed in as",
            },
            Example {
                command: "grans auth logout",
                about: "Forget the stored sign-in",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "grans looks for a Granola token in this order: --token or GRANS_TOKEN,",
                "then the sign-in stored by `grans auth login`, then the Granola desktop",
                "app's own sign-in. macOS has no third step, so `grans auth login` is",
                "required there.",
            ],
        }],
    },
    CommandHelp {
        path: "auth login",
        examples: &[
            Example {
                command: "grans auth login",
                about: "Sign in with a Google account",
            },
            Example {
                command: "grans auth login --provider microsoft",
                about: "Sign in with a Microsoft account",
            },
            Example {
                command: "grans auth login --refresh-token-stdin",
                about: "Use a refresh token piped in, with no browser",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "The browser sign-in needs someone at the keyboard to paste the final",
                "URL back. The session it creates is separate from the desktop app's, so",
                "afterwards grans works without Granola running or installed.",
            ],
        }],
    },
    CommandHelp {
        path: "auth status",
        examples: &[
            Example {
                command: "grans auth status",
                about: "The account, where the sign-in is stored, and its expiry",
            },
            Example {
                command: "grans auth status --utc",
                about: "The same, with times in UTC",
            },
        ],
        sections: &[],
    },
    CommandHelp {
        path: "auth logout",
        examples: &[Example {
            command: "grans auth logout",
            about: "Remove the sign-in stored on this machine",
        }],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "This only removes the local copy. The session stays active in Granola",
                "until it is revoked there.",
            ],
        }],
    },
    CommandHelp {
        path: "dropbox",
        examples: &[
            Example {
                command: "grans dropbox init",
                about: "Connect grans to Dropbox, once per machine",
            },
            Example {
                command: "grans dropbox push",
                about: "Upload this machine's database",
            },
            Example {
                command: "grans dropbox pull",
                about: "Download it on another machine",
            },
            Example {
                command: "grans dropbox status",
                about: "Compare the local copy with the one in Dropbox",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Shares one database between machines, so transcripts are fetched and",
                "embeddings built only once. The database lives in Apps/grans in your",
                "Dropbox.",
                "",
                "push and pull transfer nothing when the two copies are identical. They",
                "refuse when the other copy has changed since the last transfer, or when",
                "the copies differ and grans has no record of a transfer. --force",
                "overwrites the other copy anyway.",
            ],
        }],
    },
    CommandHelp {
        path: "dropbox init",
        examples: &[Example {
            command: "grans dropbox init",
            about: "Authorize grans in a browser and store the result",
        }],
        sections: &[Section {
            title: "Notes",
            lines: &["Needs someone at the keyboard to paste an authorization code."],
        }],
    },
    CommandHelp {
        path: "dropbox push",
        examples: &[
            Example {
                command: "grans dropbox push",
                about: "Upload the database unless Dropbox's copy has changed",
            },
            Example {
                command: "grans dropbox push --force",
                about: "Replace Dropbox's copy with this one regardless",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Run this after `grans sync --all` so other machines can pull the result",
                "instead of repeating the sync.",
            ],
        }],
    },
    CommandHelp {
        path: "dropbox pull",
        examples: &[
            Example {
                command: "grans dropbox pull",
                about: "Download the database unless the local copy has changed",
            },
            Example {
                command: "grans dropbox pull --force",
                about: "Replace the local copy with Dropbox's regardless",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "The download is verified before it replaces anything. If it is",
                "incomplete or corrupt, the existing database is left untouched.",
            ],
        }],
    },
    CommandHelp {
        path: "dropbox status",
        examples: &[Example {
            command: "grans dropbox status",
            about: "The local and Dropbox copies side by side",
        }],
        sections: &[],
    },
    CommandHelp {
        path: "dropbox logout",
        examples: &[Example {
            command: "grans dropbox logout",
            about: "Forget the Dropbox authorization on this machine",
        }],
        sections: &[],
    },
    CommandHelp {
        path: "update",
        examples: &[
            Example {
                command: "grans update",
                about: "Install the latest release, after asking",
            },
            Example {
                command: "grans update --check",
                about: "Only say whether a newer release exists",
            },
            Example {
                command: "grans update --wait",
                about: "For scripts: no questions, and wait for a release still building",
            },
            Example {
                command: "grans update --wait --timeout 300",
                about: "The same, giving up after 300 seconds",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &["`grans --version` prints the installed version."],
        }],
    },
    CommandHelp {
        path: "skill",
        examples: &[
            Example {
                command: "grans skill",
                about: "Print the skill",
            },
            Example {
                command: "grans skill > ~/.claude/skills/grans/SKILL.md",
                about: "Save it where an agent tool reads skills",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "The skill tells an AI agent when grans is relevant and to read",
                "`grans --help` for how to use it. It holds no usage documentation of",
                "its own, so it stays correct as commands change.",
                "",
                "grans only prints the skill. Where it belongs depends on the agent",
                "tool, so redirect it there yourself; the path above is one tool's.",
            ],
        }],
    },
];
