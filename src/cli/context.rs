use anyhow::Result;
use chrono::FixedOffset;

use crate::output::format::{detect_output_mode, OutputMode};

pub struct RunContext {
    pub output_mode: OutputMode,
    pub tz: FixedOffset,
}

impl RunContext {
    /// Create context from CLI arguments
    pub fn from_args(json: bool, no_color: bool, utc: bool) -> Result<Self> {
        if no_color {
            colored::control::set_override(false);
        }

        let output_mode = detect_output_mode(json);
        let tz = if utc {
            FixedOffset::east_opt(0).unwrap()
        } else {
            *chrono::Local::now().offset()
        };

        Ok(RunContext { output_mode, tz })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_flag_gives_zero_offset() {
        let ctx = RunContext::from_args(false, false, true).unwrap();
        assert_eq!(ctx.tz, FixedOffset::east_opt(0).unwrap());
    }

    #[test]
    fn default_gives_local_offset() {
        let ctx = RunContext::from_args(false, false, false).unwrap();
        let local_offset = *chrono::Local::now().offset();
        assert_eq!(ctx.tz, local_offset);
    }
}
