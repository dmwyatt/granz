//! Dropbox API client for file operations.
//!
//! Implements upload, download, and metadata operations using the Dropbox HTTP API.
//! Files under 150 MB use single-request uploads; larger files use chunked upload sessions.

use log::debug;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Instant;

use super::transfer::{self, ProgressFn, ProgressReader, UPLOAD_TIMEOUT};
use super::{SyncError, SyncResult};

const UPLOAD_URL: &str = "https://content.dropboxapi.com/2/files/upload";
const UPLOAD_SESSION_START_URL: &str =
    "https://content.dropboxapi.com/2/files/upload_session/start";
const UPLOAD_SESSION_APPEND_URL: &str =
    "https://content.dropboxapi.com/2/files/upload_session/append_v2";
const UPLOAD_SESSION_FINISH_URL: &str =
    "https://content.dropboxapi.com/2/files/upload_session/finish";
const DOWNLOAD_URL: &str = "https://content.dropboxapi.com/2/files/download";
const METADATA_URL: &str = "https://api.dropboxapi.com/2/files/get_metadata";

/// Dropbox's single-request upload limit is 150 MB.
const UPLOAD_SINGLE_LIMIT: u64 = 150 * 1024 * 1024;

/// Chunk size for upload sessions (8 MB — must be a multiple of 4 MB per Dropbox docs).
const UPLOAD_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// File metadata from Dropbox.
#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
pub struct FileMetadata {
    pub name: String,
    pub path_display: Option<String>,
    pub size: u64,
    pub server_modified: String,
    #[serde(rename = ".tag")]
    pub tag: Option<String>,
    /// Dropbox's hash of the stored file, for verifying a download.
    ///
    /// Optional so that a response without it still deserializes; callers that
    /// require verification reject its absence themselves.
    pub content_hash: Option<String>,
}

/// Error response from Dropbox API.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct DropboxError {
    error_summary: Option<String>,
    error: Option<serde_json::Value>,
}

/// Upload commit parameters shared by single and session uploads.
#[derive(Serialize)]
struct UploadArg {
    path: String,
    mode: String,
    autorename: bool,
    mute: bool,
}

/// Cursor for tracking position within an upload session.
#[derive(Serialize)]
struct SessionCursor {
    session_id: String,
    offset: u64,
}

/// Dropbox API client with automatic token refresh.
pub struct DropboxClient {
    access_token: String,
    http: reqwest::blocking::Client,
}

/// Serialize a `Dropbox-API-Arg` header value.
fn serialize_arg<T: Serialize>(arg: &T) -> SyncResult<String> {
    serde_json::to_string(arg).map_err(|e| SyncError::DropboxApi(format!("serialize arg: {}", e)))
}

impl DropboxClient {
    /// Create a new client with an access token.
    pub fn new(access_token: String) -> SyncResult<Self> {
        Ok(Self {
            access_token,
            http: transfer::build_client()?,
        })
    }

    /// Upload a file to Dropbox.
    ///
    /// The `dropbox_path` should be like `/index.db` (within the app folder).
    /// Files over 150 MB are automatically uploaded using chunked upload sessions.
    pub fn upload(
        &self,
        local_path: &Path,
        dropbox_path: &str,
        on_progress: ProgressFn,
    ) -> SyncResult<FileMetadata> {
        let file_size = std::fs::metadata(local_path)?.len();
        let start = Instant::now();

        let result = if file_size <= UPLOAD_SINGLE_LIMIT {
            debug!("upload {} ({} bytes, single request)", dropbox_path, file_size);
            self.upload_single(local_path, dropbox_path, file_size, on_progress)
        } else {
            debug!("upload {} ({} bytes, chunked session)", dropbox_path, file_size);
            self.upload_chunked(local_path, dropbox_path, file_size, on_progress)
        };

        if result.is_ok() {
            debug!(
                "  upload complete: {}",
                transfer::describe_throughput(file_size, start.elapsed())
            );
        }

        result
    }

    /// Upload a file in a single request (for files <= 150 MB).
    fn upload_single(
        &self,
        local_path: &Path,
        dropbox_path: &str,
        file_size: u64,
        on_progress: ProgressFn,
    ) -> SyncResult<FileMetadata> {
        let arg = UploadArg {
            path: dropbox_path.to_string(),
            mode: "overwrite".to_string(),
            autorename: false,
            mute: true,
        };

        let arg_json = serialize_arg(&arg)?;

        // Stream from the file rather than reading it in: this keeps up to
        // 150 MB out of memory and lets reqwest's pull of the body drive the
        // progress report.
        let file = std::fs::File::open(local_path)?;
        let body = reqwest::blocking::Body::sized(ProgressReader::new(file, on_progress), file_size);

        let response = self
            .http
            .post(UPLOAD_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Dropbox-API-Arg", arg_json)
            .header("Content-Type", "application/octet-stream")
            .timeout(UPLOAD_TIMEOUT)
            .body(body)
            .send()
            .map_err(|e| transfer::transport_error(format!("upload of {}", dropbox_path), e))?;

        self.handle_response(response)
    }

    /// Upload a file using chunked upload sessions (for files > 150 MB).
    fn upload_chunked(
        &self,
        local_path: &Path,
        dropbox_path: &str,
        file_size: u64,
        mut on_progress: ProgressFn,
    ) -> SyncResult<FileMetadata> {
        let mut file = std::fs::File::open(local_path)?;
        let mut buf = vec![0u8; UPLOAD_CHUNK_SIZE];
        let mut offset: u64 = 0;

        // Start: send the first chunk
        let bytes_read = file.read(&mut buf).map_err(SyncError::Io)?;
        let session_id = self.upload_session_start(&buf[..bytes_read])?;
        offset += bytes_read as u64;
        on_progress(offset);

        // Append: send middle chunks
        while offset < file_size {
            let bytes_read = file.read(&mut buf).map_err(SyncError::Io)?;
            if bytes_read == 0 {
                break;
            }

            let remaining = file_size - offset - bytes_read as u64;
            if remaining == 0 {
                // Last chunk — commit the session
                on_progress(file_size);
                return self.upload_session_finish(
                    &session_id,
                    offset,
                    &buf[..bytes_read],
                    dropbox_path,
                );
            }

            self.upload_session_append(&session_id, offset, &buf[..bytes_read])?;
            offset += bytes_read as u64;
            on_progress(offset);
        }

        // Edge case: file size was an exact multiple of chunk size, finish with empty body
        on_progress(file_size);
        self.upload_session_finish(&session_id, offset, &[], dropbox_path)
    }

    /// Start an upload session and return the session ID.
    fn upload_session_start(&self, data: &[u8]) -> SyncResult<String> {
        #[derive(Serialize)]
        struct StartArg {
            close: bool,
        }

        let arg_json = serialize_arg(&StartArg { close: false })?;

        let response = self
            .http
            .post(UPLOAD_SESSION_START_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Dropbox-API-Arg", &arg_json)
            .header("Content-Type", "application/octet-stream")
            .timeout(UPLOAD_TIMEOUT)
            .body(data.to_vec())
            .send()
            .map_err(|e| transfer::transport_error("upload session start", e))?;

        #[derive(Deserialize)]
        struct StartResult {
            session_id: String,
        }

        let result: StartResult = self.handle_response(response)?;
        Ok(result.session_id)
    }

    /// Append data to an upload session.
    fn upload_session_append(
        &self,
        session_id: &str,
        offset: u64,
        data: &[u8],
    ) -> SyncResult<()> {
        #[derive(Serialize)]
        struct AppendArg {
            cursor: SessionCursor,
            close: bool,
        }

        let arg = AppendArg {
            cursor: SessionCursor {
                session_id: session_id.to_string(),
                offset,
            },
            close: false,
        };

        let arg_json = serialize_arg(&arg)?;

        let response = self
            .http
            .post(UPLOAD_SESSION_APPEND_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Dropbox-API-Arg", &arg_json)
            .header("Content-Type", "application/octet-stream")
            .timeout(UPLOAD_TIMEOUT)
            .body(data.to_vec())
            .send()
            .map_err(|e| {
                transfer::transport_error(format!("upload of chunk at offset {}", offset), e)
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(self.parse_error(status, &body));
        }

        Ok(())
    }

    /// Finish an upload session and commit the file.
    fn upload_session_finish(
        &self,
        session_id: &str,
        offset: u64,
        data: &[u8],
        dropbox_path: &str,
    ) -> SyncResult<FileMetadata> {
        #[derive(Serialize)]
        struct FinishArg {
            cursor: SessionCursor,
            commit: UploadArg,
        }

        let arg = FinishArg {
            cursor: SessionCursor {
                session_id: session_id.to_string(),
                offset,
            },
            commit: UploadArg {
                path: dropbox_path.to_string(),
                mode: "overwrite".to_string(),
                autorename: false,
                mute: true,
            },
        };

        let arg_json = serialize_arg(&arg)?;

        let response = self
            .http
            .post(UPLOAD_SESSION_FINISH_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Dropbox-API-Arg", &arg_json)
            .header("Content-Type", "application/octet-stream")
            .timeout(UPLOAD_TIMEOUT)
            .body(data.to_vec())
            .send()
            .map_err(|e| {
                transfer::transport_error(format!("upload commit for {}", dropbox_path), e)
            })?;

        self.handle_response(response)
    }

    /// Download a small file from Dropbox into memory.
    ///
    /// Only for files whose size is known to be modest, such as sync metadata.
    /// Use [`DropboxClient::download_to_writer`] for databases.
    pub fn download(&self, dropbox_path: &str) -> SyncResult<Vec<u8>> {
        let mut buffer = Vec::new();
        self.download_to_writer(dropbox_path, &mut buffer, &mut |_| {})?;
        Ok(buffer)
    }

    /// Stream a file from Dropbox into `writer`, reporting the running byte count.
    ///
    /// Returns the number of bytes written.
    ///
    /// Streaming keeps the file out of memory and, because reqwest bounds each
    /// read rather than the request as a whole, lets a transfer run as long as
    /// the connection keeps delivering data.
    pub fn download_to_writer<W: Write>(
        &self,
        dropbox_path: &str,
        writer: &mut W,
        on_progress: &mut dyn FnMut(u64),
    ) -> SyncResult<u64> {
        #[derive(Serialize)]
        struct DownloadArg {
            path: String,
        }

        let arg_json = serialize_arg(&DownloadArg {
            path: dropbox_path.to_string(),
        })?;

        debug!("POST {} (download {})", DOWNLOAD_URL, dropbox_path);
        let start = Instant::now();

        let mut response = self
            .http
            .post(DOWNLOAD_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Dropbox-API-Arg", arg_json)
            .send()
            .map_err(|e| {
                debug!("  network error after {:?}: {}", start.elapsed(), e);
                transfer::transport_error(format!("download of {}", dropbox_path), e)
            })?;

        let status = response.status();
        debug!("  response: {} in {:?}", status, start.elapsed());

        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(self.parse_error(status, &body));
        }

        // Mirror the running count so a failed transfer can still report how far
        // it got, which is what distinguishes a mid-transfer drop from a stall
        // that never delivered anything.
        let body_start = Instant::now();
        let mut transferred = 0u64;
        let result = {
            let mut track = |bytes: u64| {
                transferred = bytes;
                on_progress(bytes);
            };
            transfer::stream_with_progress(&mut response, writer, &mut track)
        };

        let elapsed = body_start.elapsed();
        match result {
            Ok(written) => {
                debug!(
                    "  body complete: {}",
                    transfer::describe_throughput(written, elapsed)
                );
                Ok(written)
            }
            Err(e) => {
                debug!(
                    "  body failed after {}",
                    transfer::describe_throughput(transferred, elapsed)
                );
                Err(transfer::stream_error(
                    format!("download of {}", dropbox_path),
                    e,
                ))
            }
        }
    }

    /// Get metadata for a file on Dropbox.
    ///
    /// Returns `None` if the file doesn't exist.
    pub fn get_metadata(&self, dropbox_path: &str) -> SyncResult<Option<FileMetadata>> {
        #[derive(Serialize)]
        struct MetadataArg {
            path: String,
        }

        let arg = MetadataArg {
            path: dropbox_path.to_string(),
        };

        debug!("POST {} (metadata for {})", METADATA_URL, dropbox_path);
        let start = Instant::now();

        let response = self
            .http
            .post(METADATA_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&arg)
            .send()
            .map_err(|e| {
                debug!("  network error after {:?}: {}", start.elapsed(), e);
                transfer::transport_error(format!("metadata lookup for {}", dropbox_path), e)
            })?;

        let status = response.status();
        debug!("  response: {} in {:?}", status, start.elapsed());
        let body = response.text().unwrap_or_default();

        if status.as_u16() == 409 {
            // Check if it's a "not found" error
            if body.contains("not_found") {
                debug!("  {} does not exist on Dropbox", dropbox_path);
                return Ok(None);
            }
        }

        if !status.is_success() {
            return Err(self.parse_error(status, &body));
        }

        let metadata: FileMetadata = serde_json::from_str(&body)
            .map_err(|e| SyncError::DropboxApi(format!("parse metadata: {}", e)))?;

        Ok(Some(metadata))
    }

    fn handle_response<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::blocking::Response,
    ) -> SyncResult<T> {
        let status = response.status();
        let body = response.text().unwrap_or_default();

        if !status.is_success() {
            return Err(self.parse_error(status, &body));
        }

        serde_json::from_str(&body)
            .map_err(|e| SyncError::DropboxApi(format!("parse response: {}", e)))
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> SyncError {
        if let Ok(err) = serde_json::from_str::<DropboxError>(body) {
            let summary = err.error_summary.unwrap_or_else(|| status.to_string());
            return SyncError::DropboxApi(summary);
        }
        SyncError::DropboxApi(format!("HTTP {}: {}", status, body))
    }
}

/// Parse a Dropbox server_modified timestamp to Unix timestamp.
pub fn parse_dropbox_time(server_modified: &str) -> Option<u64> {
    // Format: "2025-01-27T10:30:00Z"
    chrono::DateTime::parse_from_rfc3339(server_modified)
        .ok()
        .map(|dt| dt.timestamp() as u64)
}

/// Format a Unix timestamp for display in local time.
pub fn format_timestamp(ts: u64) -> String {
    use chrono::{DateTime, Local};
    let dt = DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dropbox_time() {
        let ts = parse_dropbox_time("2025-01-27T10:30:00Z");
        assert!(ts.is_some());

        // Verify the timestamp is reasonable (after 2025-01-01)
        assert!(ts.unwrap() > 1735689600);
    }

    #[test]
    fn test_parse_dropbox_time_invalid() {
        let ts = parse_dropbox_time("invalid");
        assert!(ts.is_none());
    }

    #[test]
    fn test_format_timestamp() {
        // 2025-01-27T10:30:00Z
        let formatted = format_timestamp(1737973800);
        assert!(formatted.contains("2025"));
    }
}
