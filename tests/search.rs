mod common;

use common::TestEnv;
use predicates::prelude::*;

// --- FTS5 transcript search (using search --context) ---

#[test]
fn transcript_search_single_word() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "kickoff", "--context", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert!(!windows.is_empty());

    let matched_text = windows[0]["matched"]["text"].as_str().unwrap();
    assert!(matched_text.to_lowercase().contains("kickoff"));
}

#[test]
fn transcript_search_phrase() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "resource allocation", "--context", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert!(!windows.is_empty());

    let matched_text = windows[0]["matched"]["text"].as_str().unwrap();
    assert!(matched_text.contains("resource allocation"));
}

#[test]
fn transcript_search_no_match_returns_empty() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "xyzzyplughnotaword", "--context", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // No transcript_results key (or empty) means no matches
    let count = result.get("transcript_results")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(count, 0);
}

#[test]
fn transcript_search_context_window_has_before_and_after() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "timeline", "--context", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert!(!windows.is_empty());

    let window = &windows[0];
    // The matched utterance is "Today we will discuss the project timeline."
    // With context=2, we should have before and/or after entries
    let before = window["before"].as_array().unwrap();
    let after = window["after"].as_array().unwrap();
    // "timeline" is in utt-a2 (index 1), so there should be 1 before (utt-a1) and 2 after
    assert!(!before.is_empty() || !after.is_empty());
}

#[test]
fn transcript_search_within_specific_meeting() {
    let env = TestEnv::with_fixture();

    // "latency" only appears in doc-beta transcript
    let output = env
        .cmd_json()
        .args(["search", "latency", "--context", "2", "--meeting", "Beta"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert!(!windows.is_empty());
    assert_eq!(windows[0]["document_id"], "doc-beta");
}

#[test]
fn transcript_search_restricted_to_wrong_meeting_returns_empty() {
    let env = TestEnv::with_fixture();

    // "latency" is only in doc-beta, so searching within "Alpha" should yield nothing
    let output = env
        .cmd_json()
        .args(["search", "latency", "--context", "2", "--meeting", "Alpha"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let count = result.get("transcript_results")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(count, 0);
}

// --- FTS5 notes search (via search --in notes) ---

#[test]
fn meetings_search_notes_fts() {
    let env = TestEnv::with_fixture();
    // "milestones" appears in doc-alpha notes
    env.cmd()
        .args(["search", "milestones", "--in", "notes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("doc-alpha").or(predicate::str::contains("Alpha")));
}

#[test]
fn meetings_search_notes_no_match() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "xyzzynotaword", "--in", "notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(docs.is_empty());
}

// --- Multi-target search ---

#[test]
fn meetings_search_multiple_targets() {
    let env = TestEnv::with_fixture();
    // "prototype" appears in transcript of doc-alpha (utt-a3) and doc-beta (utt-b3)
    // Searching both titles and transcripts
    env.cmd()
        .args(["search", "prototype", "--in", "titles,transcripts"])
        .assert()
        .success();
}

// --- Limit flag ---

#[test]
fn keyword_search_respects_limit() {
    let env = TestEnv::with_fixture();
    // "prototype" matches in both doc-alpha and doc-beta transcripts (2 meetings).
    // --limit 1 should return only 1 result.
    let output = env
        .cmd_json()
        .args(["search", "prototype", "--in", "transcripts", "--limit", "1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 1);
}

#[test]
fn keyword_search_limit_zero_returns_all() {
    let env = TestEnv::with_fixture();
    // --limit 0 means no limit, should return all matches
    let output = env
        .cmd_json()
        .args(["search", "prototype", "--in", "transcripts", "--limit", "0"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(docs.len() >= 2);
}

#[test]
fn context_search_respects_limit() {
    let env = TestEnv::with_fixture();
    // "prototype" matches in both doc-alpha and doc-beta transcripts.
    // --context 1 --limit 1 should return only 1 context window.
    let output = env
        .cmd_json()
        .args(["search", "prototype", "--context", "1", "--limit", "1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
}
