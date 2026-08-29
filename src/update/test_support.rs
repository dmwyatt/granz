//! Test doubles shared by the update module's unit tests.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use super::github::{BuildStatus, GitHubApi, Release, WorkflowRun};
use super::{UpdateError, UpdateResult};

/// A `GitHubApi` that replays queued responses and counts the calls it got.
///
/// Both queues are consumed in order; running one dry panics, which keeps a
/// test from silently passing because a call it expected never happened.
pub struct MockGitHubApi {
    build_status_responses: RefCell<VecDeque<UpdateResult<BuildStatus>>>,
    release_responses: RefCell<VecDeque<UpdateResult<Release>>>,
    pub build_status_calls: Cell<usize>,
    pub release_calls: Cell<usize>,
}

impl MockGitHubApi {
    pub fn new(
        build_status_responses: Vec<UpdateResult<BuildStatus>>,
        release_responses: Vec<UpdateResult<Release>>,
    ) -> Self {
        Self {
            build_status_responses: RefCell::new(build_status_responses.into()),
            release_responses: RefCell::new(release_responses.into()),
            build_status_calls: Cell::new(0),
            release_calls: Cell::new(0),
        }
    }

    /// A mock that only ever answers build-status checks.
    pub fn build_statuses(responses: Vec<UpdateResult<BuildStatus>>) -> Self {
        Self::new(responses, vec![])
    }
}

impl GitHubApi for MockGitHubApi {
    fn fetch_latest_release(&self, _token: Option<&str>) -> UpdateResult<Release> {
        self.release_calls.set(self.release_calls.get() + 1);
        self.release_responses
            .borrow_mut()
            .pop_front()
            .expect("Unexpected call to fetch_latest_release")
    }

    fn check_build_status(&self, _token: Option<&str>) -> UpdateResult<BuildStatus> {
        self.build_status_calls
            .set(self.build_status_calls.get() + 1);
        self.build_status_responses
            .borrow_mut()
            .pop_front()
            .expect("Unexpected call to check_build_status")
    }
}

pub fn test_release() -> Release {
    Release {
        tag_name: "v2025.1.30".to_string(),
        name: Some("2025.1.30 (abc1234)".to_string()),
        body: None,
        published_at: Some("2025-01-30T12:00:00Z".to_string()),
        assets: vec![],
    }
}

pub fn in_progress_run() -> WorkflowRun {
    WorkflowRun {
        id: 12345,
        name: Some("Release".to_string()),
        status: "in_progress".to_string(),
        conclusion: None,
        html_url: "https://github.com/test/test/actions/runs/12345".to_string(),
        created_at: "2025-01-30T12:00:00Z".to_string(),
    }
}

pub fn completed_run() -> WorkflowRun {
    WorkflowRun {
        status: "completed".to_string(),
        conclusion: Some("success".to_string()),
        ..in_progress_run()
    }
}

pub fn failed_run(conclusion: &str) -> WorkflowRun {
    WorkflowRun {
        status: "completed".to_string(),
        conclusion: Some(conclusion.to_string()),
        ..in_progress_run()
    }
}

/// A 404 from the GitHub API, which is how a private repo looks without auth.
pub fn not_found_error() -> UpdateError {
    UpdateError::GitHubApi("HTTP 404: Not Found".to_string())
}
