//! Dropbox content hashing.
//!
//! Dropbox reports a `content_hash` for every stored file, letting a download be
//! checked against the bytes the server actually holds rather than inferring
//! integrity from a byte count. The algorithm is defined at
//! <https://www.dropbox.com/developers/reference/content-hash>: split the file
//! into 4 MiB blocks, SHA-256 each block, concatenate those digests in order,
//! and SHA-256 the result.

use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Dropbox hashes the file in 4 MiB blocks.
const BLOCK_SIZE: usize = 4 * 1024 * 1024;

/// Compute the Dropbox content hash of a file on disk.
///
/// Reads the file in its own pass rather than hashing during the upload, which
/// would mean threading a shared hasher through a request body that reqwest
/// owns. The extra read costs a second or so against a transfer measured in
/// minutes.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = ContentHasher::new();
    let mut buffer = vec![0u8; BLOCK_SIZE];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buffer[..bytes_read]);
    }
}

/// Computes a Dropbox content hash incrementally.
///
/// Data may be fed in pieces of any size; block boundaries are handled
/// internally, so a download can be hashed as it streams past.
pub struct ContentHasher {
    /// Hashes the block currently being filled.
    block: Sha256,
    /// Bytes accumulated into `block` so far.
    block_len: usize,
    /// Hashes the concatenated per-block digests.
    digests: Sha256,
}

impl ContentHasher {
    pub fn new() -> Self {
        Self {
            block: Sha256::new(),
            block_len: 0,
            digests: Sha256::new(),
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let room = BLOCK_SIZE - self.block_len;
            let take = room.min(data.len());

            self.block.update(&data[..take]);
            self.block_len += take;
            data = &data[take..];

            if self.block_len == BLOCK_SIZE {
                self.finish_block();
            }
        }
    }

    /// Fold the completed block's digest into the outer hash.
    fn finish_block(&mut self) {
        let digest = self.block.finalize_reset();
        self.digests.update(digest);
        self.block_len = 0;
    }

    /// Finish hashing and return the lowercase hex digest.
    pub fn finish(mut self) -> String {
        // A trailing partial block still contributes a digest. An empty input
        // contributes none, leaving the hash of an empty concatenation.
        if self.block_len > 0 {
            self.finish_block();
        }

        format!("{:x}", self.digests.finalize())
    }
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Wraps a writer, hashing everything written through it.
///
/// Lets a streaming download verify its content without a second pass over the
/// file.
pub struct HashingWriter<W> {
    inner: W,
    hasher: ContentHasher,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: ContentHasher::new(),
        }
    }

    /// Return the wrapped writer and the hash of everything written.
    pub fn finish(self) -> (W, String) {
        (self.inner, self.hasher.finish())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Hash only what the inner writer accepted, so a short write cannot
        // desynchronise the digest from the bytes on disk.
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected digests were computed independently with Python's hashlib
    // following the published algorithm, not with this implementation.
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const HELLO_WORLD: &str = "bc62d4b80d9e36da29c16c5d4d9f11731f36052c72401a76c23c0fb5a9b74423";
    const ONE_FULL_BLOCK: &str = "907a506cf5e706bda5c7a29b43c9c65d8344bd2fa2f22339b359c214812af5a1";
    const ONE_BLOCK_PLUS_ONE: &str =
        "c53f160bb0f52f97d686989961c455b5bba18dbca70d2cdc402eb63cdddf7b4d";
    const TWO_FULL_BLOCKS: &str =
        "caa2b7a097746dcd56cbddd09bb9e5b5ab1c3a9a518a63453f07fb799e7839ec";

    fn hash_all(data: &[u8]) -> String {
        let mut hasher = ContentHasher::new();
        hasher.update(data);
        hasher.finish()
    }

    /// Feed data in fixed-size pieces, mimicking a body arriving over a socket.
    fn hash_in_pieces(data: &[u8], piece: usize) -> String {
        let mut hasher = ContentHasher::new();
        for chunk in data.chunks(piece) {
            hasher.update(chunk);
        }
        hasher.finish()
    }

    #[test]
    fn hashes_an_empty_input() {
        assert_eq!(hash_all(b""), EMPTY);
    }

    #[test]
    fn hashes_a_short_input() {
        assert_eq!(hash_all(b"hello world"), HELLO_WORLD);
    }

    #[test]
    fn hashes_exactly_one_block() {
        assert_eq!(hash_all(&vec![b'a'; BLOCK_SIZE]), ONE_FULL_BLOCK);
    }

    #[test]
    fn hashes_one_byte_past_a_block_boundary() {
        assert_eq!(hash_all(&vec![b'b'; BLOCK_SIZE + 1]), ONE_BLOCK_PLUS_ONE);
    }

    #[test]
    fn hashes_two_whole_blocks() {
        assert_eq!(hash_all(&vec![b'c'; 2 * BLOCK_SIZE]), TWO_FULL_BLOCKS);
    }

    /// The hash must not depend on how the data was chopped up on the way in,
    /// which is the property that makes streaming safe.
    #[test]
    fn piece_size_does_not_change_the_hash() {
        let data = vec![b'b'; BLOCK_SIZE + 1];

        for piece in [1, 7, 1000, 64 * 1024, BLOCK_SIZE - 1, BLOCK_SIZE] {
            assert_eq!(
                hash_in_pieces(&data, piece),
                ONE_BLOCK_PLUS_ONE,
                "piece size {} changed the digest",
                piece
            );
        }
    }

    /// Hashing a file must agree with hashing the same bytes in memory, so an
    /// upload check compares like with like.
    #[test]
    fn hash_file_matches_the_in_memory_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("payload.bin");
        let data = vec![b'b'; BLOCK_SIZE + 1];
        std::fs::write(&path, &data).unwrap();

        assert_eq!(hash_file(&path).unwrap(), ONE_BLOCK_PLUS_ONE);
        assert_eq!(hash_file(&path).unwrap(), hash_all(&data));
    }

    #[test]
    fn hash_file_handles_an_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();

        assert_eq!(hash_file(&path).unwrap(), EMPTY);
    }

    #[test]
    fn hash_file_reports_a_missing_file() {
        assert!(hash_file(Path::new("no/such/file.bin")).is_err());
    }

    #[test]
    fn hashing_writer_passes_bytes_through_and_hashes_them() {
        let mut writer = HashingWriter::new(Vec::new());

        writer.write_all(b"hello ").unwrap();
        writer.write_all(b"world").unwrap();
        let (inner, hash) = writer.finish();

        assert_eq!(inner, b"hello world");
        assert_eq!(hash, HELLO_WORLD);
    }

    #[test]
    fn hashing_writer_handles_a_block_spanning_write() {
        let data = vec![b'b'; BLOCK_SIZE + 1];
        let mut writer = HashingWriter::new(Vec::new());

        for chunk in data.chunks(64 * 1024) {
            writer.write_all(chunk).unwrap();
        }
        let (inner, hash) = writer.finish();

        assert_eq!(inner.len(), data.len());
        assert_eq!(hash, ONE_BLOCK_PLUS_ONE);
    }
}
