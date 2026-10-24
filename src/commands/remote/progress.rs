//! Transfer progress for `grans dropbox push` and `pull`.

use std::io::{self, IsTerminal};

use indicatif::{ProgressBar, ProgressStyle};

use crate::sync::transfer::{ProgressFn, no_progress};

/// Byte-count progress bar for a transfer in either direction, shown only when
/// stderr is a terminal.
pub(super) struct TransferProgress {
    bar: Option<ProgressBar>,
}

impl TransferProgress {
    pub(super) fn new(total: u64) -> Self {
        if !io::stderr().is_terminal() {
            return Self { bar: None };
        }

        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "[grans] {bytes}/{total_bytes} [{bar:30}] {percent}% {bytes_per_sec}, ETA {eta}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=> "),
        );
        Self { bar: Some(pb) }
    }

    pub(super) fn set(&self, bytes: u64) {
        if let Some(ref pb) = self.bar {
            pb.set_position(bytes);
        }
    }

    /// An owned reporter for callers that hand progress off to the transport.
    ///
    /// The handle is a cheap clone of the same bar, so updates through it land
    /// on the bar this value still controls and clears.
    pub(super) fn reporter(&self) -> ProgressFn {
        match self.bar {
            Some(ref pb) => {
                let handle = pb.clone();
                Box::new(move |bytes| handle.set_position(bytes))
            }
            None => no_progress(),
        }
    }
}

/// Clear the bar however the transfer ends, so a failure message is not printed
/// underneath a stale progress line.
impl Drop for TransferProgress {
    fn drop(&mut self) {
        if let Some(ref pb) = self.bar {
            pb.finish_and_clear();
        }
    }
}
