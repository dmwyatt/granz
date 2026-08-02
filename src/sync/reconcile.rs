//! Deciding whether a transfer may overwrite the copy on the other side.
//!
//! The question is which side holds changes the other has not seen. Timestamps
//! answer it badly: Dropbox's `server_modified` records when an upload landed,
//! which is always later than the modification time of the file that was
//! uploaded, so a freshly pushed remote copy looks newer than the local file it
//! came from. Comparing content instead removes the ambiguity, using the
//! content both sides shared at the last successful sync as the reference
//! point.

/// What a transfer should do, given how the two sides relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDecision {
    /// Both sides already hold the same content; there is nothing to move.
    UpToDate,
    /// The target has not changed since the last sync, so overwriting it is safe.
    Proceed,
    /// The target changed since the last sync, so overwriting would discard it.
    Diverged,
    /// The two sides differ and there is no record of a previous sync, so
    /// neither can be shown to supersede the other.
    Unknown,
}

/// Decide whether `source` may overwrite `target`.
///
/// Hashes are Dropbox content hashes. `synced` is the content both sides held
/// at the last successful sync, absent if they have never synced.
pub fn decide(source: &str, target: Option<&str>, synced: Option<&str>) -> TransferDecision {
    let Some(target) = target else {
        // Nothing on the far side to lose.
        return TransferDecision::Proceed;
    };

    if target == source {
        return TransferDecision::UpToDate;
    }

    match synced {
        Some(synced) if synced == target => TransferDecision::Proceed,
        Some(_) => TransferDecision::Diverged,
        None => TransferDecision::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::TransferDecision::*;
    use super::*;

    const A: &str = "aaaa";
    const B: &str = "bbbb";
    const C: &str = "cccc";

    #[test]
    fn an_absent_target_is_always_safe_to_write() {
        assert_eq!(decide(A, None, None), Proceed);
        assert_eq!(decide(A, None, Some(B)), Proceed);
    }

    #[test]
    fn identical_sides_need_no_transfer() {
        assert_eq!(decide(A, Some(A), None), UpToDate);
        assert_eq!(decide(A, Some(A), Some(A)), UpToDate);
        assert_eq!(decide(A, Some(A), Some(B)), UpToDate);
    }

    #[test]
    fn an_untouched_target_may_be_overwritten() {
        // The target still holds what both sides shared last sync, so only the
        // source has moved on.
        assert_eq!(decide(B, Some(A), Some(A)), Proceed);
    }

    #[test]
    fn a_target_that_moved_on_its_own_is_a_conflict() {
        // Neither side holds the synced content any more: both changed.
        assert_eq!(decide(B, Some(C), Some(A)), Diverged);
    }

    #[test]
    fn differing_sides_without_a_sync_record_are_unknown() {
        assert_eq!(decide(A, Some(B), None), Unknown);
    }

    /// The bug this module exists to fix: pushing, then pushing again with
    /// nothing changed locally, must not report a conflict.
    #[test]
    fn pushing_twice_with_no_local_change_is_up_to_date() {
        let after_push = A;

        // Local and remote both hold what the first push stored.
        assert_eq!(
            decide(after_push, Some(after_push), Some(after_push)),
            UpToDate
        );
    }

    /// The same in the other direction: pulling twice must not claim the local
    /// copy is newer just because it was written more recently.
    #[test]
    fn pulling_twice_with_no_remote_change_is_up_to_date() {
        let after_pull = A;

        assert_eq!(
            decide(after_pull, Some(after_pull), Some(after_pull)),
            UpToDate
        );
    }

    /// A machine that pushed, then synced new data locally, may push again.
    #[test]
    fn local_changes_after_a_sync_may_be_pushed() {
        let synced = A;
        let local_after_new_data = B;

        assert_eq!(
            decide(local_after_new_data, Some(synced), Some(synced)),
            Proceed
        );
    }

    /// A second machine pushed since we last synced, so our push would discard
    /// its work.
    #[test]
    fn a_push_from_another_machine_blocks_ours() {
        let we_synced = A;
        let other_machine_pushed = B;
        let our_local = C;

        assert_eq!(
            decide(our_local, Some(other_machine_pushed), Some(we_synced)),
            Diverged
        );
    }

    /// Deciding is symmetric: swapping which side is the source turns a push
    /// question into a pull question.
    #[test]
    fn the_decision_is_symmetric_between_push_and_pull() {
        let synced = A;
        let changed = B;

        // Local changed, remote untouched: push proceeds, pull would discard.
        assert_eq!(decide(changed, Some(synced), Some(synced)), Proceed);
        assert_eq!(decide(synced, Some(changed), Some(synced)), Diverged);
    }
}
