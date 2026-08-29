//! Test doubles shared by the update module's unit tests.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use anyhow::Result;

use super::fetch::{AuthProvider, PromptProvider};
use super::github::{BuildStatus, GitHubApi, Release, WorkflowRun};
use super::{UpdateError, UpdateResult};

/// A `GitHubApi` that replays queued responses and counts the calls it got.
///
/// Every queue is consumed in order; running one dry panics, which keeps a
/// test from silently passing because a call it expected never happened.
pub struct MockGitHubApi {
    build_status_responses: RefCell<VecDeque<UpdateResult<BuildStatus>>>,
    run_responses: RefCell<VecDeque<UpdateResult<WorkflowRun>>>,
    release_responses: RefCell<VecDeque<UpdateResult<Release>>>,
    pub build_status_calls: Cell<usize>,
    pub run_calls: Cell<usize>,
    pub release_calls: Cell<usize>,
    /// The run ids `fetch_workflow_run` was asked for, in order.
    pub run_ids_requested: RefCell<Vec<u64>>,
}

impl MockGitHubApi {
    pub fn new(
        build_status_responses: Vec<UpdateResult<BuildStatus>>,
        release_responses: Vec<UpdateResult<Release>>,
    ) -> Self {
        Self {
            build_status_responses: RefCell::new(build_status_responses.into()),
            run_responses: RefCell::new(VecDeque::new()),
            release_responses: RefCell::new(release_responses.into()),
            build_status_calls: Cell::new(0),
            run_calls: Cell::new(0),
            release_calls: Cell::new(0),
            run_ids_requested: RefCell::new(Vec::new()),
        }
    }

    /// A mock that only ever answers polls of a single run.
    pub fn runs(responses: Vec<UpdateResult<WorkflowRun>>) -> Self {
        Self::new(vec![], vec![]).with_runs(responses)
    }

    /// A mock that only ever answers release fetches.
    pub fn releases(responses: Vec<UpdateResult<Release>>) -> Self {
        Self::new(vec![], responses)
    }

    /// Queue the answers to `fetch_workflow_run`.
    pub fn with_runs(self, responses: Vec<UpdateResult<WorkflowRun>>) -> Self {
        *self.run_responses.borrow_mut() = responses.into();
        self
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

    fn fetch_workflow_run(&self, id: u64, _token: Option<&str>) -> UpdateResult<WorkflowRun> {
        self.run_calls.set(self.run_calls.get() + 1);
        self.run_ids_requested.borrow_mut().push(id);
        self.run_responses
            .borrow_mut()
            .pop_front()
            .expect("Unexpected call to fetch_workflow_run")
    }
}

/// An `AuthProvider` with a fixed answer for the gh CLI token.
pub struct MockAuthProvider {
    pub token: Option<String>,
}

impl AuthProvider for MockAuthProvider {
    fn get_gh_cli_token(&self) -> Option<String> {
        self.token.clone()
    }
}

/// A `PromptProvider` that answers from a script and panics if asked more
/// than the script covers, so a test can assert that no prompt happened.
pub struct MockPromptProvider {
    responses: RefCell<Vec<bool>>,
    pub call_count: Cell<usize>,
}

impl MockPromptProvider {
    pub fn new(responses: Vec<bool>) -> Self {
        Self {
            responses: RefCell::new(responses),
            call_count: Cell::new(0),
        }
    }
}

impl PromptProvider for MockPromptProvider {
    fn prompt_yes_no(&self, _message: &str) -> Result<bool> {
        let idx = self.call_count.get();
        self.call_count.set(idx + 1);
        let responses = self.responses.borrow();
        match responses.get(idx) {
            Some(answer) => Ok(*answer),
            None => panic!("Unexpected prompt call (call #{})", idx + 1),
        }
    }
}

/// The release that `completed_run()` published: its title carries the run's sha.
pub fn test_release() -> Release {
    Release {
        tag_name: "v2025.1.30".to_string(),
        name: Some("2025.1.30 (abc1234)".to_string()),
        body: None,
        published_at: Some("2025-01-30T12:00:00Z".to_string()),
        assets: vec![],
    }
}

/// The release before `test_release()`, built from a different commit.
pub fn stale_release() -> Release {
    Release {
        tag_name: "v2025.1.29".to_string(),
        name: Some("2025.1.29 (0000000)".to_string()),
        published_at: Some("2025-01-29T09:00:00Z".to_string()),
        ..test_release()
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
        head_sha: "abc1234def5678abc1234def5678abc1234def56".to_string(),
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
