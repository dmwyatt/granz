//! GitHub API client for fetching release information.

use serde::Deserialize;

use super::{UpdateError, UpdateResult};

const RELEASE_URL: &str = "https://api.github.com/repos/dmwyatt/granz/releases/latest";

/// Trait for GitHub API operations, allowing for mocking in tests.
pub trait GitHubApi {
    /// Fetch the latest release from GitHub.
    fn fetch_latest_release(&self, token: Option<&str>) -> UpdateResult<Release>;

    /// Check the status of the Release workflow.
    fn check_build_status(&self, token: Option<&str>) -> UpdateResult<BuildStatus>;

    /// Fetch one workflow run by id.
    fn fetch_workflow_run(&self, id: u64, token: Option<&str>) -> UpdateResult<WorkflowRun>;
}

/// Real GitHub API client that makes HTTP requests.
#[derive(Default)]
pub struct RealGitHubApi;

impl GitHubApi for RealGitHubApi {
    fn fetch_latest_release(&self, token: Option<&str>) -> UpdateResult<Release> {
        fetch_latest_release(token)
    }

    fn check_build_status(&self, token: Option<&str>) -> UpdateResult<BuildStatus> {
        check_build_status(token)
    }

    fn fetch_workflow_run(&self, id: u64, token: Option<&str>) -> UpdateResult<WorkflowRun> {
        fetch_workflow_run(id, token)
    }
}

/// GitHub release metadata.
#[derive(Deserialize, Debug, Clone)]
pub struct Release {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<Asset>,
}

/// Release asset metadata.
#[derive(Deserialize, Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    /// API URL for downloading (works for private repos with auth).
    pub url: String,
    /// Direct browser URL (only works for public repos).
    pub browser_download_url: String,
    /// Digest in format "sha256:abc123..."
    pub digest: Option<String>,
}

impl Asset {
    /// Extract SHA256 hash from digest field.
    pub fn sha256(&self) -> Option<&str> {
        self.digest.as_ref().and_then(|d| d.strip_prefix("sha256:"))
    }
}

/// Fetch the latest release from GitHub.
///
/// If `token` is provided, it will be used for authentication.
/// Otherwise, an unauthenticated request is made.
pub fn fetch_latest_release(token: Option<&str>) -> UpdateResult<Release> {
    let response = send_get(RELEASE_URL, token)?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(UpdateError::NotFound {
            has_token: token.is_some(),
        });
    }

    parse_response(response, "response")
}

/// Send a GET to the GitHub API, authenticated when a token is on hand.
fn send_get(url: &str, token: Option<&str>) -> UpdateResult<reqwest::blocking::Response> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("grans/{}", env!("GRANS_VERSION")))
        .build()?;

    let mut request = client.get(url);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    Ok(request.send()?)
}

/// Turn a GitHub API response into `T`, or into an error naming the HTTP status.
fn parse_response<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
    what: &str,
) -> UpdateResult<T> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(UpdateError::GitHubApi(format!("HTTP {}: {}", status, body)));
    }

    response
        .json()
        .map_err(|e| UpdateError::GitHubApi(format!("Failed to parse {}: {}", what, e)))
}

/// Format a GitHub API timestamp (RFC 3339, UTC) for display in local time.
///
/// A timestamp that does not parse is shown as GitHub sent it rather than
/// hidden; it is display text, and the raw form is still readable.
pub fn format_timestamp(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}

/// Find the asset matching the given name.
pub fn find_asset<'a>(release: &'a Release, asset_name: &str) -> Option<&'a Asset> {
    release.assets.iter().find(|a| a.name == asset_name)
}

/// GitHub Actions workflow run metadata.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: u64,
    pub name: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    pub created_at: String,
    /// The commit the run built.
    pub head_sha: String,
}

/// Response from GitHub Actions workflow runs API.
#[derive(Deserialize, Debug)]
pub struct WorkflowRunsResponse {
    pub workflow_runs: Vec<WorkflowRun>,
}

/// Status of the release build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStatus {
    /// No recent build found
    Idle,
    /// Build is currently running
    InProgress(WorkflowRun),
    /// Build completed successfully
    Completed(WorkflowRun),
    /// Build failed
    Failed(WorkflowRun),
}

/// Runs of the Release workflow on main, newest first. Asking for the
/// workflow's own runs keeps issue-triggered workflows from crowding it out.
const RELEASE_RUNS_URL: &str = "https://api.github.com/repos/dmwyatt/granz/actions/workflows/release.yml/runs?branch=main&per_page=1";
const RUN_URL: &str = "https://api.github.com/repos/dmwyatt/granz/actions/runs";

/// Fetch the most recent Release workflow run from GitHub Actions.
pub fn fetch_workflow_runs(token: Option<&str>) -> UpdateResult<WorkflowRunsResponse> {
    parse_response(send_get(RELEASE_RUNS_URL, token)?, "workflow runs")
}

/// Fetch one workflow run by id.
pub fn fetch_workflow_run(id: u64, token: Option<&str>) -> UpdateResult<WorkflowRun> {
    let url = format!("{}/{}", RUN_URL, id);
    parse_response(send_get(&url, token)?, "workflow run")
}

/// Check the status of the Release workflow.
///
/// Looks at the most recent Release workflow run and returns its status.
pub fn check_build_status(token: Option<&str>) -> UpdateResult<BuildStatus> {
    let runs = fetch_workflow_runs(token)?;

    Ok(runs
        .workflow_runs
        .into_iter()
        .next()
        .map_or(BuildStatus::Idle, BuildStatus::from_run))
}

impl BuildStatus {
    /// Classify a run by the status and conclusion GitHub reports for it.
    ///
    /// status can be: queued, in_progress, completed, waiting, pending, requested.
    /// conclusion (when completed): success, failure, cancelled, skipped, etc.
    pub fn from_run(run: WorkflowRun) -> Self {
        match run.status.as_str() {
            "completed" => match run.conclusion.as_deref() {
                Some("success") => BuildStatus::Completed(run),
                _ => BuildStatus::Failed(run),
            },
            "queued" | "in_progress" | "waiting" | "pending" | "requested" => {
                BuildStatus::InProgress(run)
            }
            _ => BuildStatus::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_sha256_extraction() {
        let asset = Asset {
            name: "test".to_string(),
            size: 100,
            url: "https://api.github.com/repos/test/test/releases/assets/123".to_string(),
            browser_download_url: "https://example.com".to_string(),
            digest: Some("sha256:abc123def456".to_string()),
        };
        assert_eq!(asset.sha256(), Some("abc123def456"));
    }

    #[test]
    fn test_asset_sha256_none_when_missing() {
        let asset = Asset {
            name: "test".to_string(),
            size: 100,
            url: "https://api.github.com/repos/test/test/releases/assets/123".to_string(),
            browser_download_url: "https://example.com".to_string(),
            digest: None,
        };
        assert_eq!(asset.sha256(), None);
    }

    #[test]
    fn test_asset_sha256_none_when_wrong_prefix() {
        let asset = Asset {
            name: "test".to_string(),
            size: 100,
            url: "https://api.github.com/repos/test/test/releases/assets/123".to_string(),
            browser_download_url: "https://example.com".to_string(),
            digest: Some("md5:abc123".to_string()),
        };
        assert_eq!(asset.sha256(), None);
    }

    #[test]
    fn test_find_asset() {
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            name: None,
            body: None,
            published_at: None,
            assets: vec![
                Asset {
                    name: "grans-linux-x86_64".to_string(),
                    size: 1000,
                    url: "https://api.github.com/repos/test/test/releases/assets/1".to_string(),
                    browser_download_url: "https://example.com/linux".to_string(),
                    digest: None,
                },
                Asset {
                    name: "grans-macos-aarch64".to_string(),
                    size: 1100,
                    url: "https://api.github.com/repos/test/test/releases/assets/2".to_string(),
                    browser_download_url: "https://example.com/macos".to_string(),
                    digest: None,
                },
            ],
        };

        let found = find_asset(&release, "grans-linux-x86_64");
        assert!(found.is_some());
        assert_eq!(found.unwrap().size, 1000);

        let not_found = find_asset(&release, "grans-windows-x86_64.exe");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_workflow_run_deserialization() {
        let json = r#"{
            "id": 12345,
            "name": "Release",
            "status": "in_progress",
            "conclusion": null,
            "html_url": "https://github.com/dmwyatt/granz/actions/runs/12345",
            "created_at": "2025-01-30T14:23:00Z",
            "head_sha": "abc1234def5678"
        }"#;

        let run: WorkflowRun = serde_json::from_str(json).unwrap();
        assert_eq!(run.id, 12345);
        assert_eq!(run.name, Some("Release".to_string()));
        assert_eq!(run.status, "in_progress");
        assert_eq!(run.conclusion, None);
    }

    #[test]
    fn test_workflow_runs_response_deserialization() {
        let json = r#"{
            "total_count": 2,
            "workflow_runs": [
                {
                    "id": 12345,
                    "name": "Release",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/dmwyatt/granz/actions/runs/12345",
                    "created_at": "2025-01-30T14:23:00Z",
                    "head_sha": "abc1234def5678"
                },
                {
                    "id": 12344,
                    "name": "CI",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/dmwyatt/granz/actions/runs/12344",
                    "created_at": "2025-01-30T14:00:00Z",
                    "head_sha": "abc1234def5678"
                }
            ]
        }"#;

        let response: WorkflowRunsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.workflow_runs.len(), 2);
        assert_eq!(response.workflow_runs[0].name, Some("Release".to_string()));
    }

    #[test]
    fn test_build_status_from_completed_success() {
        let run = WorkflowRun {
            id: 1,
            name: Some("Release".to_string()),
            status: "completed".to_string(),
            conclusion: Some("success".to_string()),
            html_url: "https://example.com".to_string(),
            created_at: "2025-01-30T14:23:00Z".to_string(),
            head_sha: "abc1234def5678".to_string(),
        };

        match BuildStatus::Completed(run.clone()) {
            BuildStatus::Completed(r) => assert_eq!(r.id, 1),
            _ => panic!("Expected Completed status"),
        }
    }

    #[test]
    fn test_build_status_from_in_progress() {
        let run = WorkflowRun {
            id: 2,
            name: Some("Release".to_string()),
            status: "in_progress".to_string(),
            conclusion: None,
            html_url: "https://example.com".to_string(),
            created_at: "2025-01-30T14:23:00Z".to_string(),
            head_sha: "abc1234def5678".to_string(),
        };

        match BuildStatus::InProgress(run.clone()) {
            BuildStatus::InProgress(r) => assert_eq!(r.id, 2),
            _ => panic!("Expected InProgress status"),
        }
    }

    #[test]
    fn test_build_status_equality() {
        assert_eq!(BuildStatus::Idle, BuildStatus::Idle);

        let run1 = WorkflowRun {
            id: 1,
            name: Some("Release".to_string()),
            status: "completed".to_string(),
            conclusion: Some("success".to_string()),
            html_url: "https://example.com".to_string(),
            created_at: "2025-01-30T14:23:00Z".to_string(),
            head_sha: "abc1234def5678".to_string(),
        };

        let run2 = run1.clone();
        assert_eq!(BuildStatus::Completed(run1), BuildStatus::Completed(run2));
    }

    fn run_with(status: &str, conclusion: Option<&str>) -> WorkflowRun {
        WorkflowRun {
            id: 1,
            name: Some("Release".to_string()),
            status: status.to_string(),
            conclusion: conclusion.map(str::to_string),
            html_url: "https://example.com".to_string(),
            created_at: "2025-01-30T14:23:00Z".to_string(),
            head_sha: "abc1234def5678".to_string(),
        }
    }

    #[test]
    fn test_from_run_classifies_by_status_and_conclusion() {
        assert!(matches!(
            BuildStatus::from_run(run_with("completed", Some("success"))),
            BuildStatus::Completed(_)
        ));
        assert!(matches!(
            BuildStatus::from_run(run_with("completed", Some("cancelled"))),
            BuildStatus::Failed(_)
        ));
        assert!(matches!(
            BuildStatus::from_run(run_with("queued", None)),
            BuildStatus::InProgress(_)
        ));
        assert_eq!(
            BuildStatus::from_run(run_with("mystery", None)),
            BuildStatus::Idle
        );
    }

    #[test]
    fn test_format_timestamp_shows_local_time() {
        let formatted = format_timestamp("2025-01-30T14:23:00Z");
        // Local time, so only the shape is fixed: YYYY-MM-DD HH:MM:SS
        assert_eq!(formatted.len(), 19);
        assert_eq!(&formatted[10..11], " ");
    }

    #[test]
    fn test_format_timestamp_keeps_unparseable_input() {
        assert_eq!(format_timestamp("not-a-timestamp"), "not-a-timestamp");
    }
}
