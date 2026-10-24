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
        path: "remote",
        examples: &[
            Example {
                command: "grans remote init dropbox",
                about: "Connect grans to Dropbox, once per machine",
            },
            Example {
                command: "grans remote init google-drive client_secret.json",
                about: "Or to Google Drive, through your own OAuth client",
            },
            Example {
                command: "grans remote push",
                about: "Upload this machine's database",
            },
            Example {
                command: "grans remote pull",
                about: "Download it on another machine",
            },
            Example {
                command: "grans remote status",
                about: "Compare the local copy with the remote one",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Shares one database between machines through Dropbox or Google Drive,",
                "so transcripts are fetched and embeddings built only once. Connect each",
                "machine to the same remote with `grans remote init`, then push from one",
                "and pull on the others.",
                "",
                "push and pull transfer nothing when the two copies are identical. They",
                "refuse when the other copy has changed since the last transfer, or when",
                "the copies differ and grans has no record of a transfer. --force",
                "overwrites the other copy anyway.",
            ],
        }],
    },
    CommandHelp {
        path: "remote init",
        examples: &[
            Example {
                command: "grans remote init dropbox",
                about: "Keep the shared copy in Dropbox",
            },
            Example {
                command: "grans remote init google-drive client_secret.json",
                about: "Keep it in Google Drive, through your own OAuth client",
            },
        ],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "Connecting to a different remote than before replaces the old",
                "connection, and grans forgets what it knew about the copy there.",
            ],
        }],
    },
    CommandHelp {
        path: "remote init dropbox",
        examples: &[Example {
            command: "grans remote init dropbox",
            about: "Authorize grans in a browser and store the result",
        }],
        sections: &[Section {
            title: "Notes",
            lines: &[
                "The database lives in Apps/grans in your Dropbox, and grans can see",
                "nothing else there. Needs someone at the keyboard to paste an",
                "authorization code.",
            ],
        }],
    },
    CommandHelp {
        path: "remote init google-drive",
        examples: &[Example {
            command: "grans remote init google-drive client_secret.json",
            about: "Log in with the client you created",
        }],
        sections: &[
            Section {
                title: "Setup",
                lines: &[
                    "grans brings no Google app of its own, so you create one for it in",
                    "your Google account. It takes about ten minutes, once: every machine",
                    "then uses the file it produces. Sign in to console.cloud.google.com",
                    "with the Google account whose Drive should hold the database.",
                    "",
                    "1. Create a project",
                    "   Click the project picker at the top left, beside the Google Cloud",
                    "   logo, then New project. Name it grans and click Create. If it asks",
                    "   for a billing account, any will do: the Drive API is free. When",
                    "   the notification says the project exists, click Select project.",
                    "   The picker should now read grans.",
                    "",
                    "2. Turn on the Drive API",
                    "   Open APIs & Services from the navigation menu at the top left and",
                    "   click Enable APIs and services. Search for Google Drive API, open",
                    "   the result with exactly that name (not Drive Activity or Drive",
                    "   MCP), and click Enable.",
                    "",
                    "3. Describe the app",
                    "   In the APIs & Services menu on the left, click OAuth consent",
                    "   screen, then Get started. Enter grans as the app name and pick",
                    "   your address as the support email, choose External, give your",
                    "   address as the contact, tick the agreement to the Google API",
                    "   Services User Data Policy, and click Create.",
                    "",
                    "4. Give it Drive access",
                    "   In the menu on the left, open Data Access and click Add or remove",
                    "   scopes. Type drive.file into the panel's filter, tick the Google",
                    "   Drive API row (.../auth/drive.file), and click Update. Then click",
                    "   Save at the bottom of the page; nothing is kept until you do.",
                    "",
                    "5. Publish it",
                    "   Google will not publish an app without a home page and a privacy",
                    "   policy link. Nobody but you will see them, so the grans repository",
                    "   serves for both. Open Branding, enter",
                    "   https://github.com/dmwyatt/granz as the Application home page and",
                    "   as the Application privacy policy link, click Add domain under",
                    "   Authorized domains, enter github.com, and click Save. Then open",
                    "   Audience, click Publish app, and Confirm. The status should read",
                    "   In production. An app left in Testing loses its login after 7",
                    "   days, and this command would have to be run again every week.",
                    "",
                    "6. Create the client",
                    "   Open Clients, click Create client, choose Desktop app as the",
                    "   application type, name it grans, and click Create. In the dialog",
                    "   that follows, click Download JSON before closing it: Google never",
                    "   shows the client secret again. If the dialog closed first, delete",
                    "   the client and create another. The downloaded file,",
                    "   client_secret_....json, is what this command takes.",
                    "",
                    "7. Connect grans",
                    "   Run this command with the downloaded file. Your browser opens on",
                    "   Google's account chooser: pick the same account, then approve the",
                    "   permissions page, which asks to let grans see and change only the",
                    "   Drive files it uses. The terminal then says it is connected.",
                    "",
                    "Publishing needs no review from Google, because grans asks only for",
                    "drive.file, which Google rates as non-sensitive.",
                ],
            },
            Section {
                title: "Notes",
                lines: &[
                    "Use the same client file on every machine. Google shows grans only",
                    "the files created through it, so a machine with another client finds",
                    "no database to pull.",
                    "",
                    "grans sees the folder named grans that it creates, and nothing else",
                    "in your Drive.",
                    "",
                    "New settings can take a few minutes to reach Google's login. If this",
                    "command fails right after the setup, wait a few minutes and retry.",
                    "",
                    "The login opens a browser on this machine. On a machine without one,",
                    "open the printed address elsewhere and paste back the address the",
                    "browser lands on, even though that page fails to load.",
                ],
            },
        ],
    },
    CommandHelp {
        path: "remote push",
        examples: &[
            Example {
                command: "grans remote push",
                about: "Upload the database unless the remote copy has changed",
            },
            Example {
                command: "grans remote push --force",
                about: "Replace the remote copy with this one regardless",
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
        path: "remote pull",
        examples: &[
            Example {
                command: "grans remote pull",
                about: "Download the database unless the local copy has changed",
            },
            Example {
                command: "grans remote pull --force",
                about: "Replace the local copy with the remote one regardless",
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
        path: "remote status",
        examples: &[Example {
            command: "grans remote status",
            about: "The local and remote copies side by side",
        }],
        sections: &[],
    },
    CommandHelp {
        path: "remote logout",
        examples: &[Example {
            command: "grans remote logout",
            about: "Forget the remote authorization on this machine",
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
