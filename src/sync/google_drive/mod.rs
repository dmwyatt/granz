//! Google Drive, reached with an OAuth client the user registered.
//!
//! The shared copy lives in a folder named `grans` that grans creates. Drive
//! identifies files by ID rather than path and lets several files share a name,
//! so every operation first looks the file up by name within that folder. The
//! `drive.file` scope limits those lookups to files grans created.

pub mod loopback;
pub mod oauth;

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use log::debug;
use serde::Deserialize;
use serde_json::json;

use super::remote::{Provider, Remote, RemoteFile};
use super::transfer::{self, ProgressFn, ProgressReader, UPLOAD_TIMEOUT};
use super::{SyncError, SyncResult};

const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

const FOLDER_NAME: &str = "grans";
const FOLDER_MIME_TYPE: &str = "application/vnd.google-apps.folder";

/// The fields grans reads about a stored file.
const FILE_FIELDS: &str = "id,size,modifiedTime,sha256Checksum";

/// A file as Drive describes it.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    /// Bytes stored, as a decimal string. Absent for folders.
    size: Option<String>,
    modified_time: Option<String>,
    /// Drive's SHA-256 of the stored bytes, in hex.
    sha256_checksum: Option<String>,
}

impl DriveFile {
    fn into_remote_file(self, name: &str) -> SyncResult<RemoteFile> {
        let size = self
            .size
            .as_deref()
            .ok_or_else(|| api_error(format!("Google Drive reported no size for {}", name)))?
            .parse()
            .map_err(|e| {
                api_error(format!(
                    "Google Drive reported a bad size for {}: {}",
                    name, e
                ))
            })?;

        Ok(RemoteFile {
            size,
            content_hash: self.sha256_checksum.map(|hash| hash.to_ascii_lowercase()),
            modified: self.modified_time.as_deref().and_then(parse_drive_time),
        })
    }
}

#[derive(Deserialize)]
struct FileList {
    files: Vec<DriveFile>,
}

fn parse_file_list(body: &str) -> SyncResult<Vec<DriveFile>> {
    serde_json::from_str::<FileList>(body)
        .map(|list| list.files)
        .map_err(|e| api_error(format!("parse file list: {}", e)))
}

/// The one file a lookup by name found, if any.
///
/// Drive lets several files share a name, and grans cannot tell which of them
/// is the shared copy, so more than one is an error for the user to resolve.
fn single(mut files: Vec<DriveFile>, name: &str) -> SyncResult<Option<DriveFile>> {
    match files.len() {
        0 | 1 => Ok(files.pop()),
        n => Err(api_error(format!(
            "found {} items named {} that grans created, so it cannot tell which is the \
             shared copy. In Google Drive, delete all but the one to keep, then try again.",
            n, name
        ))),
    }
}

/// Quote a value as a string in Drive's query language.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Parse an RFC 3339 `modifiedTime` to a Unix timestamp.
fn parse_drive_time(modified: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(modified)
        .ok()
        .map(|dt| dt.timestamp() as u64)
}

fn api_error(message: String) -> SyncError {
    SyncError::Api {
        provider: Provider::GoogleDrive,
        message,
    }
}

/// The error body Drive sends with a failed request.
#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

fn parse_error(status: reqwest::StatusCode, body: &str) -> SyncError {
    match serde_json::from_str::<ErrorBody>(body) {
        Ok(parsed) => api_error(format!("{} (HTTP {})", parsed.error.message, status)),
        Err(_) => api_error(format!("HTTP {}: {}", status, body)),
    }
}

/// Read a response body, or turn a failed status into an error.
fn read_body(response: reqwest::blocking::Response) -> SyncResult<String> {
    let status = response.status();

    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(parse_error(status, &body));
    }

    Ok(response.text()?)
}

fn read_json<T: for<'de> Deserialize<'de>>(response: reqwest::blocking::Response) -> SyncResult<T> {
    serde_json::from_str(&read_body(response)?)
        .map_err(|e| api_error(format!("parse response: {}", e)))
}

/// Google Drive API client.
pub struct GoogleDriveClient {
    access_token: String,
    http: reqwest::blocking::Client,
}

impl GoogleDriveClient {
    pub fn new(access_token: String) -> SyncResult<Self> {
        Ok(Self {
            access_token,
            http: transfer::build_client()?,
        })
    }

    /// Run a file search, returning every match.
    fn search(&self, query: &str, fields: &str) -> SyncResult<Vec<DriveFile>> {
        debug!("GET {} (q: {})", FILES_URL, query);
        let start = Instant::now();

        let response = self
            .http
            .get(FILES_URL)
            .bearer_auth(&self.access_token)
            .query(&[
                ("q", query),
                ("fields", &format!("files({})", fields)),
                ("spaces", "drive"),
            ])
            .send()
            .map_err(|e| transfer::transport_error("Google Drive file search", e))?;
        debug!("  response: {} in {:?}", response.status(), start.elapsed());

        parse_file_list(&read_body(response)?)
    }

    /// The ID of grans' folder, if it exists.
    ///
    /// Searched for anywhere in Drive rather than at the top, so the folder can
    /// be moved without grans losing it.
    fn folder_id(&self) -> SyncResult<Option<String>> {
        let query = format!(
            "name = {} and mimeType = {} and trashed = false",
            quote(FOLDER_NAME),
            quote(FOLDER_MIME_TYPE)
        );
        Ok(single(self.search(&query, "id")?, FOLDER_NAME)?.map(|f| f.id))
    }

    fn create_folder(&self) -> SyncResult<String> {
        debug!("POST {} (create folder {})", FILES_URL, FOLDER_NAME);

        let response = self
            .http
            .post(FILES_URL)
            .bearer_auth(&self.access_token)
            .query(&[("fields", "id")])
            .json(&json!({ "name": FOLDER_NAME, "mimeType": FOLDER_MIME_TYPE }))
            .send()
            .map_err(|e| transfer::transport_error("Google Drive folder creation", e))?;

        let folder: DriveFile = read_json(response)?;
        Ok(folder.id)
    }

    /// The file named `name` in grans' folder, if there is one.
    fn find_in(&self, folder_id: &str, name: &str) -> SyncResult<Option<DriveFile>> {
        let query = format!(
            "name = {} and {} in parents and mimeType != {} and trashed = false",
            quote(name),
            quote(folder_id),
            quote(FOLDER_MIME_TYPE)
        );
        single(self.search(&query, FILE_FIELDS)?, name)
    }

    fn find(&self, name: &str) -> SyncResult<Option<DriveFile>> {
        match self.folder_id()? {
            Some(folder_id) => self.find_in(&folder_id, name),
            None => Ok(None),
        }
    }

    /// Open a resumable upload session and return its URI.
    ///
    /// Replacing an existing file keeps its ID, so the folder never holds a
    /// moment with no copy, or with two.
    fn start_upload(
        &self,
        existing: Option<&DriveFile>,
        folder_id: &str,
        name: &str,
        size: u64,
    ) -> SyncResult<String> {
        let request = match existing {
            Some(file) => self
                .http
                .patch(format!("{}/{}", UPLOAD_URL, file.id))
                .json(&json!({})),
            None => self
                .http
                .post(UPLOAD_URL)
                .json(&json!({ "name": name, "parents": [folder_id] })),
        };

        let response = request
            .bearer_auth(&self.access_token)
            .query(&[("uploadType", "resumable"), ("fields", FILE_FIELDS)])
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", size)
            .send()
            .map_err(|e| {
                transfer::transport_error(format!("Google Drive upload of {}", name), e)
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(parse_error(status, &body));
        }

        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|uri| uri.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| api_error("upload session started without a session URI".to_string()))
    }

    /// Send the whole file to an upload session in one request.
    fn send_upload(
        &self,
        session_uri: &str,
        local_path: &Path,
        name: &str,
        size: u64,
        on_progress: ProgressFn,
    ) -> SyncResult<DriveFile> {
        // Streaming from the file keeps it out of memory and lets reqwest's
        // pull of the body drive the progress report.
        let file = std::fs::File::open(local_path)?;
        let body = reqwest::blocking::Body::sized(ProgressReader::new(file, on_progress), size);

        let response = self
            .http
            .put(session_uri)
            .timeout(UPLOAD_TIMEOUT)
            .body(body)
            .send()
            .map_err(|e| {
                transfer::transport_error(format!("Google Drive upload of {}", name), e)
            })?;

        read_json(response)
    }
}

impl Remote for GoogleDriveClient {
    fn provider(&self) -> Provider {
        Provider::GoogleDrive
    }

    fn stat(&self, name: &str) -> SyncResult<Option<RemoteFile>> {
        self.find(name)?
            .map(|file| file.into_remote_file(name))
            .transpose()
    }

    fn upload(
        &self,
        local_path: &Path,
        name: &str,
        on_progress: ProgressFn,
    ) -> SyncResult<RemoteFile> {
        let size = std::fs::metadata(local_path)?.len();
        let folder_id = match self.folder_id()? {
            Some(id) => id,
            None => self.create_folder()?,
        };
        let existing = self.find_in(&folder_id, name)?;

        debug!("upload {} ({} bytes, resumable session)", name, size);
        let start = Instant::now();

        let session_uri = self.start_upload(existing.as_ref(), &folder_id, name, size)?;
        let stored = self.send_upload(&session_uri, local_path, name, size, on_progress)?;

        debug!(
            "  upload complete: {}",
            transfer::describe_throughput(size, start.elapsed())
        );

        stored.into_remote_file(name)
    }

    fn download_to_writer(
        &self,
        name: &str,
        writer: &mut dyn Write,
        on_progress: &mut dyn FnMut(u64),
    ) -> SyncResult<u64> {
        let file = self
            .find(name)?
            .ok_or_else(|| api_error(format!("{} not found in the grans folder", name)))?;

        debug!(
            "GET {}/{}?alt=media (download {})",
            FILES_URL, file.id, name
        );
        let start = Instant::now();

        let mut response = self
            .http
            .get(format!("{}/{}", FILES_URL, file.id))
            .bearer_auth(&self.access_token)
            .query(&[("alt", "media")])
            .send()
            .map_err(|e| {
                transfer::transport_error(format!("Google Drive download of {}", name), e)
            })?;

        let status = response.status();
        debug!("  response: {} in {:?}", status, start.elapsed());

        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(parse_error(status, &body));
        }

        let body_start = Instant::now();
        let written = transfer::stream_with_progress(&mut response, writer, on_progress)
            .map_err(|e| transfer::stream_error(format!("Google Drive download of {}", name), e))?;
        debug!(
            "  body complete: {}",
            transfer::describe_throughput(written, body_start.elapsed())
        );

        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(json: &str) -> Vec<DriveFile> {
        parse_file_list(json).unwrap()
    }

    mod file_listing {
        use super::*;

        #[test]
        fn a_stored_file_is_described_by_its_size_hash_and_modification_time() {
            let files = listing(
                r#"{"files": [{
                    "id": "f1",
                    "size": "550805504",
                    "modifiedTime": "2026-10-24T13:37:24.123Z",
                    "sha256Checksum": "abc123"
                }]}"#,
            );

            assert_eq!(
                files[0].clone().into_remote_file("grans.db").unwrap(),
                RemoteFile {
                    size: 550_805_504,
                    content_hash: Some("abc123".to_string()),
                    modified: Some(1_792_849_044),
                }
            );
        }

        /// grans compares hashes as strings, so the case Drive happens to use
        /// must not read as a different file.
        #[test]
        fn the_hash_is_compared_in_lowercase() {
            let files =
                listing(r#"{"files": [{"id": "f1", "size": "1", "sha256Checksum": "ABC123"}]}"#);

            assert_eq!(
                files[0]
                    .clone()
                    .into_remote_file("grans.db")
                    .unwrap()
                    .content_hash
                    .as_deref(),
                Some("abc123")
            );
        }

        #[test]
        fn a_file_without_a_size_is_refused() {
            let files = listing(r#"{"files": [{"id": "f1"}]}"#);

            assert!(files[0].clone().into_remote_file("grans.db").is_err());
        }

        #[test]
        fn an_empty_listing_has_no_files() {
            assert!(listing(r#"{"files": []}"#).is_empty());
        }
    }

    mod one_match {
        use super::*;

        fn file(id: &str) -> DriveFile {
            DriveFile {
                id: id.to_string(),
                size: None,
                modified_time: None,
                sha256_checksum: None,
            }
        }

        #[test]
        fn no_match_means_no_file() {
            assert_eq!(single(vec![], "grans.db").unwrap(), None);
        }

        #[test]
        fn one_match_is_the_file() {
            let found = single(vec![file("f1")], "grans.db").unwrap();

            assert_eq!(found.map(|f| f.id), Some("f1".to_string()));
        }

        /// Two machines pushing for the first time at once can each create a
        /// copy, and picking one would silently ignore the other's meetings.
        #[test]
        fn several_matches_are_refused_by_name() {
            let err = single(vec![file("f1"), file("f2")], "grans.db")
                .unwrap_err()
                .to_string();

            assert!(err.contains("2") && err.contains("grans.db"), "{}", err);
        }
    }

    mod query {
        use super::*;

        #[test]
        fn a_name_is_quoted_as_a_query_string() {
            assert_eq!(quote("grans.db"), "'grans.db'");
        }

        #[test]
        fn quotes_and_backslashes_in_a_name_are_escaped() {
            assert_eq!(quote(r"it's a\b"), r"'it\'s a\\b'");
        }
    }
}
