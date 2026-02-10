use indicatif::{ProgressBar, ProgressStyle};
use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::time::Instant;

/// Format a duration as a human-readable string.
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{}m", m)
        } else {
            format!("{}m {}s", m, s)
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h {}m", h, m)
        }
    }
}

/// Estimates remaining time using a rolling window of recent completion times.
struct EtaEstimator {
    window: VecDeque<Instant>,
    window_size: usize,
}

impl EtaEstimator {
    fn new(window_size: usize) -> Self {
        let mut window = VecDeque::with_capacity(window_size + 1);
        window.push_back(Instant::now());
        Self {
            window,
            window_size,
        }
    }

    /// Record that an item was just completed.
    fn record(&mut self) {
        self.window.push_back(Instant::now());
        // Keep window_size + 1 entries (need two timestamps to get one interval)
        while self.window.len() > self.window_size + 1 {
            self.window.pop_front();
        }
    }

    /// Estimate remaining time based on the rolling window average.
    fn estimate_remaining(&self, remaining: u64) -> Option<String> {
        if self.window.len() < 2 || remaining == 0 {
            return None;
        }

        let oldest = self.window.front()?;
        let newest = self.window.back()?;
        let elapsed = newest.duration_since(*oldest);
        let intervals = (self.window.len() - 1) as f64;
        let avg_per_item = elapsed.as_secs_f64() / intervals;
        let remaining_secs = (avg_per_item * remaining as f64).round() as u64;

        Some(format_duration(remaining_secs))
    }
}

/// Create a spinner for indeterminate-progress operations (e.g., waiting for an API response).
/// Matches the style used in `update::wait`.
pub fn create_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("[grans] {spinner} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// A progress reporter that shows a progress bar when stderr is a TTY,
/// and stays silent otherwise.
pub struct SyncProgress {
    bar: Option<ProgressBar>,
    eta: EtaEstimator,
    total: u64,
    pos: u64,
}

impl SyncProgress {
    /// Create a new progress reporter for sync operations.
    /// Returns a reporter with an active progress bar only if stderr is a TTY.
    pub fn new(total: u64) -> Self {
        let bar = if io::stderr().is_terminal() {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("[grans] {pos}/{len} [{bar:30}] {elapsed} elapsed, {msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("=> "),
            );
            pb.set_message("estimating...");
            Some(pb)
        } else {
            None
        };

        Self {
            bar,
            eta: EtaEstimator::new(10),
            total,
            pos: 0,
        }
    }

    /// Print a message above the progress bar.
    /// When a progress bar is active, this keeps the bar at the bottom.
    pub fn println(&self, msg: &str) {
        if let Some(ref pb) = self.bar {
            pb.println(msg);
        }
    }

    /// Increment the progress by one item and update the ETA.
    pub fn inc(&mut self) {
        self.pos += 1;
        self.eta.record();

        if let Some(ref pb) = self.bar {
            pb.inc(1);
            let remaining = self.total - self.pos;
            let msg = self
                .eta
                .estimate_remaining(remaining)
                .map(|eta| format!("~{} remaining", eta))
                .unwrap_or_default();
            pb.set_message(msg);
        }
    }

    /// Finish and clear the progress bar.
    pub fn finish(&self) {
        if let Some(ref pb) = self.bar {
            pb.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3599), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(5400), "1h 30m");
        assert_eq!(format_duration(7200), "2h");
    }

    #[test]
    fn test_eta_estimator_no_data() {
        let est = EtaEstimator::new(10);
        assert!(est.estimate_remaining(5).is_none());
    }

    #[test]
    fn test_eta_estimator_zero_remaining() {
        let mut est = EtaEstimator::new(10);
        est.record();
        assert!(est.estimate_remaining(0).is_none());
    }

    #[test]
    fn test_eta_estimator_produces_estimate() {
        let mut est = EtaEstimator::new(10);
        thread::sleep(Duration::from_millis(50));
        est.record();
        thread::sleep(Duration::from_millis(50));
        est.record();
        let result = est.estimate_remaining(10);
        assert!(result.is_some());
    }

    #[test]
    fn test_eta_estimator_window_rolls() {
        let mut est = EtaEstimator::new(3);
        for _ in 0..10 {
            est.record();
        }
        // window_size + 1 entries kept
        assert_eq!(est.window.len(), 4);
    }

    #[test]
    fn test_create_spinner() {
        let spinner = create_spinner("Testing...");
        spinner.finish_and_clear();
    }
}
