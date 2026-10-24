//! `grans dropbox pull`.

use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use log::debug;

use crate::db::integrity::check_pulled_database;
use crate::db::wal::{checkpoint_file, discard, replace_database};
use crate::output::format::format_size;
use crate::output::progress::create_spinner;
use crate::sync::SyncError;
use crate::sync::config::SyncConfig;
use crate::sync::content_hash::{HashKind, HashingWriter, hash_file};
use crate::sync::reconcile::{TransferDecision, decide};
use crate::sync::remote::{Remote, RemoteFile, connect};
use crate::sync::transfer::{verify_content_hash, verify_transfer_size};

use super::progress::TransferProgress;
use super::{REMOTE_DB, current_timestamp};

/// Pull the database from the remote.
pub(super) fn pull(force: bool) -> Result<()> {
    let mut config = SyncConfig::load()?;
    let remote = connect(&config)?;

    // Get local database path
    let db_path = crate::db::connection::default_db_path()?;

    let Some(stored) = remote.stat(REMOTE_DB)? else {
        println!("No database on {}", remote.provider());
        return Ok(());
    };

    let synced_hash = pull_file(
        remote.as_ref(),
        &stored,
        &db_path,
        REMOTE_DB,
        "database",
        force,
        // What the local file held when it last agreed with the remote, which
        // is not the hash the remote holds once a local rewrite that changed no
        // data (the journal mode conversion) has moved the file on.
        config.local_reference(),
    )?;

    // Record what both sides now hold, so the next sync can tell which one moved.
    config.record_sync(synced_hash);
    config.last_pull_time = Some(current_timestamp());
    config.save()?;
    println!("\nPull complete!");

    Ok(())
}

/// Pull a file, unless the local copy holds changes that are not on the remote.
///
/// `stored` is what the remote reported for `remote_name`. Returns the content
/// hash both sides hold afterwards, to record as the reference point for the
/// next sync.
fn pull_file(
    remote: &dyn Remote,
    stored: &RemoteFile,
    local_path: &Path,
    remote_name: &str,
    what: &str,
    force: bool,
    last_synced: Option<&str>,
) -> Result<String> {
    let provider = remote.provider();

    // Verification needs the hash before the transfer starts; refusing early
    // beats discovering it after moving hundreds of megabytes.
    let expected_hash =
        stored
            .content_hash
            .as_deref()
            .ok_or_else(|| SyncError::MissingContentHash {
                provider,
                path: remote_name.to_string(),
            })?;

    let local_hash = local_content_hash(provider.hash_kind(), local_path, force);

    match decide(expected_hash, local_hash.as_deref(), last_synced) {
        TransferDecision::UpToDate => {
            println!(
                "The local {} already matches {}; nothing to download.",
                what, provider
            );
            return Ok(expected_hash.to_string());
        }
        TransferDecision::Diverged if !force => {
            return Err(SyncError::ConflictLocalChanged {
                what: what.to_string(),
            }
            .into());
        }
        TransferDecision::Unknown if !force => {
            return Err(SyncError::ConflictNoSyncRecord {
                provider,
                what: what.to_string(),
            }
            .into());
        }
        _ => {}
    }

    println!("Downloading {} ({})...", what, format_size(stored.size));

    // Ensure parent directory exists
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Download into a temp file so nothing replaces a good database until every
    // check has passed.
    let temp_path = local_path.with_extension("db.tmp");
    let downloaded = download_and_verify(
        remote,
        remote_name,
        &temp_path,
        &Expected {
            size: stored.size,
            hash: expected_hash,
            what,
        },
    );

    let installed = downloaded.and_then(|()| replace_database(&temp_path, local_path));

    match installed {
        Ok(()) => {
            println!("  Downloaded to {}", local_path.display());
            Ok(expected_hash.to_string())
        }
        // Every way the download can fail to become the local database ends
        // here, installing included: a several hundred megabyte temp file left
        // behind is removed by nothing, not even `db clear --all`.
        Err(e) => {
            discard(&temp_path);
            Err(e)
        }
    }
}

/// The content hash of the local database, when one can be established.
///
/// `None` means the answer is unknown, not that the file is absent. A local
/// database too broken to open is the usual reason to be pulling in the first
/// place, so nothing about reading it may abort the command that would replace
/// it. Under `--force` the answer changes no decision, and hashing hundreds of
/// megabytes to ignore the result is worth skipping.
fn local_content_hash(kind: HashKind, local_path: &Path, force: bool) -> Option<String> {
    if force {
        return None;
    }

    // The hash has to cover whatever is still in the write-ahead log, since
    // that is part of the database the remote copy is being compared against.
    // A file that cannot be checkpointed is still worth hashing: the bytes are
    // what they are, and a hash matching neither side stops the pull just as a
    // conflict would.
    if let Err(e) = checkpoint_file(local_path) {
        debug!(
            "{} was not checkpointed before hashing: {:#}",
            local_path.display(),
            e
        );
    }

    match hash_file(kind, local_path) {
        Ok(hash) => Some(hash),
        Err(e) => {
            debug!("could not hash {}: {}", local_path.display(), e);
            None
        }
    }
}

/// What a download has to match before it is allowed to replace the database.
struct Expected<'a> {
    size: u64,
    hash: &'a str,
    what: &'a str,
}

/// Stream a remote file to `temp_path` and prove it is safe to install.
///
/// Checks run cheapest first, so a failure costs as little as possible: the byte
/// count and content hash both come free with the transfer, while the integrity
/// check has to read the file back.
fn download_and_verify(
    remote: &dyn Remote,
    remote_name: &str,
    temp_path: &Path,
    expected: &Expected<'_>,
) -> Result<()> {
    let provider = remote.provider();
    let (written, actual_hash) = stream_to_file(remote, remote_name, temp_path, expected.size)?;

    verify_transfer_size(written, expected.size, provider, expected.what)?;
    verify_content_hash(&actual_hash, expected.hash, provider, expected.what)?;

    let spinner = create_spinner(&format!("Checking {} integrity...", expected.what));
    let result = check_pulled_database(temp_path);
    spinner.finish_and_clear();

    result.with_context(|| {
        format!(
            "the {} downloaded from {} failed its integrity check; \
             the local file was left untouched",
            expected.what, provider
        )
    })
}

/// Write the download to disk, hashing as it streams, and flush it to the device.
///
/// Returns the byte count and the content hash of what was written, in the
/// remote's algorithm.
fn stream_to_file(
    remote: &dyn Remote,
    remote_name: &str,
    temp_path: &Path,
    expected_size: u64,
) -> Result<(u64, String)> {
    let progress = TransferProgress::new(expected_size);

    let file = std::fs::File::create(temp_path)?;
    let mut writer = HashingWriter::new(remote.provider().hash_kind(), BufWriter::new(file));

    let written =
        remote.download_to_writer(remote_name, &mut writer, &mut |bytes| progress.set(bytes))?;

    let (buffered, actual_hash) = writer.finish();
    let file = buffered
        .into_inner()
        .map_err(|e| anyhow::anyhow!("flushing the download to disk: {}", e))?;

    // Without this the rename can publish a file whose contents never reached
    // the device, leaving a corrupt database after a crash.
    file.sync_all()?;

    Ok((written, actual_hash))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::sync::SyncResult;
    use crate::sync::remote::Provider;
    use crate::sync::remote::fake::FakeRemote;
    use crate::sync::transfer::ProgressFn;

    const KIND: HashKind = HashKind::Dropbox;

    /// The file a pull exists to replace is often one that will not open. Its
    /// hash is still readable, and nothing about reading it may abort the
    /// command before the download starts.
    #[test]
    fn a_local_file_that_is_not_a_database_still_hashes() {
        let dir = tempfile::TempDir::new().unwrap();
        let corrupt = dir.path().join("grans.db");
        std::fs::write(&corrupt, b"this is not a SQLite database").unwrap();

        assert!(local_content_hash(KIND, &corrupt, false).is_some());
    }

    #[test]
    fn an_absent_local_database_has_no_hash() {
        let dir = tempfile::TempDir::new().unwrap();

        assert_eq!(
            local_content_hash(KIND, &dir.path().join("nothing.db"), false),
            None
        );
    }

    /// Under --force the answer changes no decision, so hundreds of megabytes
    /// are not read to produce it.
    #[test]
    fn forcing_skips_hashing_the_local_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("grans.db");
        std::fs::write(&local, b"contents").unwrap();

        assert_eq!(local_content_hash(KIND, &local, true), None);
    }

    /// Create a real grans database at `path`, told apart from others by
    /// `marker`, and return its bytes.
    fn grans_database(path: &Path, marker: &str) -> Vec<u8> {
        let conn = crate::db::connection::open_db_at_path(path).unwrap();
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('marker', ?1)",
            [marker],
        )
        .unwrap();
        drop(conn);
        checkpoint_file(path).unwrap();
        std::fs::read(path).unwrap()
    }

    /// Serves different bytes than the hash it reports, as a download corrupted
    /// in flight would.
    struct CorruptingRemote(FakeRemote);

    impl Remote for CorruptingRemote {
        fn provider(&self) -> Provider {
            self.0.provider()
        }

        fn stat(&self, name: &str) -> SyncResult<Option<RemoteFile>> {
            self.0.stat(name)
        }

        fn upload(
            &self,
            local_path: &Path,
            name: &str,
            on_progress: ProgressFn,
        ) -> SyncResult<RemoteFile> {
            self.0.upload(local_path, name, on_progress)
        }

        fn download_to_writer(
            &self,
            name: &str,
            writer: &mut dyn Write,
            on_progress: &mut dyn FnMut(u64),
        ) -> SyncResult<u64> {
            let mut contents = Vec::new();
            let written = self
                .0
                .download_to_writer(name, &mut contents, on_progress)?;
            contents[0] ^= 0xff;
            writer.write_all(&contents)?;
            Ok(written)
        }
    }

    #[test]
    fn pulling_onto_a_machine_without_a_database_installs_the_remote_copy() {
        let dir = tempfile::TempDir::new().unwrap();
        let pushed = grans_database(&dir.path().join("pushed.db"), "pushed");
        let remote = FakeRemote::with_file(REMOTE_DB, &pushed);
        let stored = remote.stat(REMOTE_DB).unwrap().unwrap();
        let local = dir.path().join("grans.db");

        pull_file(&remote, &stored, &local, REMOTE_DB, "database", false, None).unwrap();

        assert_eq!(std::fs::read(&local).unwrap(), pushed);
    }

    #[test]
    fn a_corrupted_download_leaves_the_local_database_untouched() {
        let dir = tempfile::TempDir::new().unwrap();
        let pushed = grans_database(&dir.path().join("pushed.db"), "pushed");
        let remote = CorruptingRemote(FakeRemote::with_file(REMOTE_DB, &pushed));
        let stored = remote.stat(REMOTE_DB).unwrap().unwrap();
        let local = dir.path().join("grans.db");
        let ours = grans_database(&local, "ours");

        let result = pull_file(&remote, &stored, &local, REMOTE_DB, "database", true, None);

        assert_eq!(
            (result.is_err(), std::fs::read(&local).unwrap()),
            (true, ours)
        );
    }
}
