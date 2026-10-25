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
}
