use indicatif::{ProgressBar, ProgressStyle};

/// Create a progress bar for embedding operations.
pub fn embedding_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[grans] Embedding {pos}/{len} chunks [{bar:30}] {eta}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb
}
