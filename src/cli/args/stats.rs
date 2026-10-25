use clap::{Args, Subcommand};

use crate::cli::help;
use crate::stats::time::Period;

/// Which meetings a statistic covers. Global, so it can follow the
/// subcommand: `grans stats meetings --date last-month`.
#[derive(Args, Debug)]
pub struct StatsPeriod {
    /// Only meetings from this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
    #[arg(long, global = true)]
    pub from: Option<String>,

    /// Only meetings before this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
    #[arg(long, global = true)]
    pub to: Option<String>,

    /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
    #[arg(long, global = true)]
    pub date: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum StatsAction {
    /// One row per meeting: when, how long, who was invited, and who spoke
    #[command(after_long_help = help::for_command("stats meetings"))]
    Meetings,

    /// Where the time goes: hours per week or month, workday edges, free
    /// time, meeting sizes, and when in the week meetings happen
    #[command(after_long_help = help::for_command("stats time"))]
    Time {
        /// Group by week or by month
        #[arg(long, value_enum, default_value = "month")]
        by: Period,
    },

    /// Recurring meetings: hours, cadence, invite list, punctuality, and
    /// your share of the talk, per series
    #[command(after_long_help = help::for_command("stats series"))]
    Series {
        /// How many series to show, most hours first (0 for all)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Who you meet with: one-on-ones, who organizes, outside your
    /// organization, how invitations are answered, who is invited together
    #[command(after_long_help = help::for_command("stats people"))]
    People {
        /// How many people to list, most meetings first (0 for all)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}
