use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use log::debug;
use rand::Rng;
use thiserror::Error;

use super::types::{
    ApiPanel, GetDocumentsRequest, GetDocumentsResponse, GetDocumentPanelsRequest,
    GetPanelTemplatesRequest, GetPeopleRequest, GetRecipesRequest, GetRecipesResponse,
    GetSelectedCalendarsRequest, GetSelectedCalendarsResponse, GetTranscriptRequest,
    RefreshCalendarEventsRequest, RefreshCalendarEventsResponse, TranscriptResponse,
};
use crate::models::{
    CalendarEvent, Document, PanelTemplate, Person, TranscriptUtterance,
};

const API_V1_URL: &str = "https://api.granola.ai/v1";
const API_V2_URL: &str = "https://api.granola.ai/v2";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const CLIENT_VERSION: &str = "6.518.0";

/// Safely slice a string at UTF-8 character boundaries.
/// Returns a substring from `start` to `end` byte positions, adjusted to valid char boundaries.
fn safe_slice(s: &str, start: usize, end: usize) -> &str {
    let start = s.floor_char_boundary(start);
    let end = s.ceil_char_boundary(end.min(s.len()));
    &s[start..end]
}

/// Truncate a string for log output, appending "..." if truncated.
fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", safe_slice(s, 0, max_len))
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Authentication failed (401). Your token may have expired. Please re-login to Granola.")]
    Unauthorized,

    #[error("Resource not found (404). The requested resource may not exist.")]
    NotFound,

    #[error("Rate limited (429). Please wait before making more requests.")]
    RateLimited,

    #[error("Server error ({0}): {1}")]
    ServerError(u16, String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

pub struct ApiClient {
    token: String,
    client: reqwest::blocking::Client,
}

impl ApiClient {
    pub fn new(token: String) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {}", e))?;

        Ok(Self { token, client })
    }

    /// Internal helper to make a POST request to the v1 API
    fn post_v1<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, ApiError> {
        let url = format!("{}/{}", API_V1_URL, endpoint);
        self.post(&url, body)
    }

    /// Internal helper to make a POST request to the v2 API
    fn post_v2<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, ApiError> {
        let url = format!("{}/{}", API_V2_URL, endpoint);
        self.post(&url, body)
    }

    /// Internal helper to make a POST request
    fn post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, ApiError> {
        let body_json = serde_json::to_string(body).unwrap_or_default();
        debug!("POST {} (body: {} bytes)", url, body_json.len());
        debug!("  request body: {}", truncate_for_log(&body_json, 200));

        let start = Instant::now();
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("X-Client-Version", CLIENT_VERSION)
            .json(body)
            .send()
            .map_err(|e| {
                debug!("  network error after {:?}: {}", start.elapsed(), e);
                ApiError::NetworkError(e.to_string())
            })?;

        let status = response.status();
        debug!("  response: {} in {:?}", status, start.elapsed());

        match status.as_u16() {
            200 => {
                // Read body as text first so we can include it in error messages
                let body = response
                    .text()
                    .map_err(|e| ApiError::InvalidResponse(format!("failed to read body: {}", e)))?;
                debug!("  response body: {} bytes", body.len());
                debug!("  response preview: {}", truncate_for_log(&body, 200));
                serde_json::from_str(&body).map_err(|e| {
                    debug!("  deserialization error: {}", e);
                    let err_str = e.to_string();

                    // Try to extract column position from serde error and show context around it
                    let context = if let Some(col_str) = err_str
                        .split("column ")
                        .nth(1)
                        .and_then(|s| s.split_whitespace().next())
                    {
                        if let Ok(col) = col_str.parse::<usize>() {
                            let start = col.saturating_sub(100);
                            let end = col + 100;
                            if start < body.len() {
                                format!(
                                    "Context around column {} (chars {}-{}):\n...{}...",
                                    col, start, end, safe_slice(&body, start, end)
                                )
                            } else {
                                format!("Column {} is beyond body length {}", col, body.len())
                            }
                        } else {
                            // Fallback: show first 500 chars
                            let preview = if body.len() > 500 {
                                format!("{}...", safe_slice(&body, 0, 500))
                            } else {
                                body.clone()
                            };
                            format!("Response body:\n{}", preview)
                        }
                    } else {
                        // Fallback: show first 500 chars
                        let preview = if body.len() > 500 {
                            format!("{}...", safe_slice(&body, 0, 500))
                        } else {
                            body.clone()
                        };
                        format!("Response body:\n{}", preview)
                    };

                    ApiError::InvalidResponse(format!("{}\n\n{}", err_str, context))
                })
            }
            401 => {
                debug!("  unauthorized (401)");
                Err(ApiError::Unauthorized)
            }
            404 => {
                debug!("  not found (404)");
                Err(ApiError::NotFound)
            }
            429 => {
                debug!("  rate limited (429)");
                Err(ApiError::RateLimited)
            }
            _ => {
                let body = response.text().unwrap_or_default();
                debug!("  server error ({}): {}", status.as_u16(), truncate_for_log(&body, 500));
                Err(ApiError::ServerError(status.as_u16(), body))
            }
        }
    }

    // ========================================================================
    // Transcript Methods
    // ========================================================================

    /// Fetch a transcript for a document
    pub fn fetch_transcript(&self, document_id: &str) -> Result<TranscriptResponse, ApiError> {
        let url = format!("{}/get-document-transcript", API_V1_URL);
        debug!("POST {} (document_id={})", url, document_id);

        let request_body = GetTranscriptRequest {
            document_id: document_id.to_string(),
        };

        let start = Instant::now();
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("X-Client-Version", CLIENT_VERSION)
            .json(&request_body)
            .send()
            .map_err(|e| {
                debug!("  network error after {:?}: {}", start.elapsed(), e);
                ApiError::NetworkError(e.to_string())
            })?;

        let status = response.status();
        debug!("  response: {} in {:?}", status, start.elapsed());

        match status.as_u16() {
            200 => {
                // API returns array directly, not wrapped in {"transcript": [...]}
                let utterances: Vec<TranscriptUtterance> = response
                    .json()
                    .map_err(|e| {
                        debug!("  deserialization error: {}", e);
                        ApiError::InvalidResponse(e.to_string())
                    })?;
                debug!("  got {} utterances", utterances.len());
                Ok(TranscriptResponse {
                    transcript: utterances,
                })
            }
            401 => {
                debug!("  unauthorized (401)");
                Err(ApiError::Unauthorized)
            }
            404 => {
                debug!("  not found (404)");
                Err(ApiError::NotFound)
            }
            429 => {
                debug!("  rate limited (429)");
                Err(ApiError::RateLimited)
            }
            _ => {
                let body = response.text().unwrap_or_default();
                debug!("  server error ({}): {}", status.as_u16(), truncate_for_log(&body, 500));
                Err(ApiError::ServerError(status.as_u16(), body))
            }
        }
    }

    // ========================================================================
    // Document Methods
    // ========================================================================

    /// Fetch all documents from the API
    pub fn get_documents(&self) -> Result<Vec<Document>, ApiError> {
        let request = GetDocumentsRequest::default();
        let response: GetDocumentsResponse = self.post_v2("get-documents", &request)?;
        Ok(response.docs)
    }

    // ========================================================================
    // People Methods
    // ========================================================================

    /// Fetch all people (contacts/workspace members) from the API
    pub fn get_people(&self) -> Result<Vec<Person>, ApiError> {
        let request = GetPeopleRequest::default();
        // get-people returns a raw array, not an object
        self.post_v1("get-people", &request)
    }

    // ========================================================================
    // Calendar Methods
    // ========================================================================

    /// Fetch selected calendars settings
    pub fn get_selected_calendars(&self) -> Result<GetSelectedCalendarsResponse, ApiError> {
        let request = GetSelectedCalendarsRequest::default();
        self.post_v1("get-selected-calendars", &request)
    }

    /// Refresh and fetch calendar events
    pub fn refresh_calendar_events(&self) -> Result<Vec<CalendarEvent>, ApiError> {
        let request = RefreshCalendarEventsRequest::default();
        let response: RefreshCalendarEventsResponse =
            self.post_v1("refresh-calendar-events", &request)?;
        Ok(response
            .results
            .map(|r| r.events)
            .unwrap_or_default())
    }

    // ========================================================================
    // Template Methods
    // ========================================================================

    /// Fetch all panel templates from the API
    pub fn get_templates(&self) -> Result<Vec<PanelTemplate>, ApiError> {
        let request = GetPanelTemplatesRequest::default();
        // get-panel-templates returns a raw array
        self.post_v1("get-panel-templates", &request)
    }

    // ========================================================================
    // Panel Methods
    // ========================================================================

    /// Fetch panels (AI-generated notes) for a document
    pub fn fetch_panels(&self, document_id: &str) -> Result<Vec<ApiPanel>, ApiError> {
        let request = GetDocumentPanelsRequest {
            document_id: document_id.to_string(),
        };
        self.post_v1("get-document-panels", &request)
    }

    // ========================================================================
    // Recipe Methods
    // ========================================================================

    /// Fetch all recipes from the API
    pub fn get_recipes(&self) -> Result<GetRecipesResponse, ApiError> {
        let request = GetRecipesRequest::default();
        self.post_v1("get-recipes", &request)
    }
}

/// Convenience function to fetch a transcript with a given token
pub fn fetch_transcript(token: &str, document_id: &str) -> Result<TranscriptResponse, ApiError> {
    let client = ApiClient::new(token.to_string())
        .map_err(|e| ApiError::NetworkError(e.to_string()))?;
    client.fetch_transcript(document_id)
}

/// Convenience function to fetch panels with a given token
pub fn fetch_panels(token: &str, document_id: &str) -> Result<Vec<ApiPanel>, ApiError> {
    let client = ApiClient::new(token.to_string())
        .map_err(|e| ApiError::NetworkError(e.to_string()))?;
    client.fetch_panels(document_id)
}

/// Sleep for the specified duration plus random jitter (0-500ms)
pub fn sleep_with_jitter(base_ms: u64) {
    let jitter: u64 = rand::thread_rng().gen_range(0..500);
    thread::sleep(Duration::from_millis(base_ms + jitter));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_display() {
        let err = ApiError::Unauthorized;
        assert!(err.to_string().contains("401"));
        assert!(err.to_string().contains("expired"));

        let err = ApiError::NotFound;
        assert!(err.to_string().contains("404"));

        let err = ApiError::RateLimited;
        assert!(err.to_string().contains("429"));
    }

    #[test]
    fn test_request_serialization() {
        let request = GetTranscriptRequest {
            document_id: "doc-123".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"document_id":"doc-123"}"#);
    }

    #[test]
    fn test_documents_request_serialization() {
        // Empty request
        let request = GetDocumentsRequest::default();
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, "{}");

        // Request with ID
        let request = GetDocumentsRequest {
            id: Some("doc-123".to_string()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"id":"doc-123"}"#);
    }

    #[test]
    fn test_people_request_serialization() {
        let request = GetPeopleRequest::default();
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_templates_request_serialization() {
        let request = GetPanelTemplatesRequest::default();
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_recipes_request_serialization() {
        let request = GetRecipesRequest::default();
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_safe_slice_ascii() {
        let s = "hello world";
        assert_eq!(safe_slice(s, 0, 5), "hello");
        assert_eq!(safe_slice(s, 6, 11), "world");
        assert_eq!(safe_slice(s, 0, 100), "hello world"); // end beyond length
    }

    #[test]
    fn test_safe_slice_utf8_multibyte() {
        // Each emoji is 4 bytes
        let s = "a😀b😀c";  // bytes: a(1) + 😀(4) + b(1) + 😀(4) + c(1) = 11 bytes

        // Slicing at byte 1 would be mid-emoji without safe_slice
        // safe_slice should adjust to valid boundaries
        let result = safe_slice(s, 0, 2);
        // floor(0)=0, ceil(2)=5 (after first emoji)
        assert_eq!(result, "a😀");

        // Slicing starting mid-emoji
        let result = safe_slice(s, 2, 6);
        // floor(2)=1 (start of emoji), ceil(6)=6 (after 'b')
        assert_eq!(result, "😀b");

        // Entire string
        assert_eq!(safe_slice(s, 0, 100), s);
    }

    #[test]
    fn test_safe_slice_empty_and_edge_cases() {
        let s = "test";
        assert_eq!(safe_slice(s, 0, 0), "");
        assert_eq!(safe_slice(s, 10, 20), ""); // start beyond length

        let empty = "";
        assert_eq!(safe_slice(empty, 0, 10), "");
    }
}
