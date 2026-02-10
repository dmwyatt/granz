mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn meetings_list_from_date_filters_correctly() {
    let env = TestEnv::with_fixture();
    // doc-alpha: 2025-06-15, doc-beta: 2025-07-20, doc-gamma: 2025-08-10
    // --from 2025-07-01 should exclude doc-alpha
    let output = env
        .cmd_json()
        .args(["list", "--from", "2025-07-01"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 2);

    let titles: Vec<&str> = docs
        .iter()
        .map(|d| d["title"].as_str().unwrap())
        .collect();
    assert!(!titles.contains(&"Project Alpha Kickoff"));
    assert!(titles.contains(&"Beta Feature Review"));
    assert!(titles.contains(&"Gamma Sprint Planning"));
}

#[test]
fn meetings_list_to_date_filters_correctly() {
    let env = TestEnv::with_fixture();
    // --to 2025-07-01 should only include doc-alpha
    let output = env
        .cmd_json()
        .args(["list", "--to", "2025-07-01"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["title"], "Project Alpha Kickoff");
}

#[test]
fn meetings_list_from_and_to_range() {
    let env = TestEnv::with_fixture();
    // --from 2025-07-01 --to 2025-08-01 should only include doc-beta
    let output = env
        .cmd_json()
        .args([
            "list", "--from", "2025-07-01", "--to", "2025-08-01",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["title"], "Beta Feature Review");
}

#[test]
fn meetings_list_from_after_all_returns_empty() {
    let env = TestEnv::with_fixture();
    // All meetings are before 2026, so --from 2026-01-01 should return nothing
    let output = env
        .cmd_json()
        .args(["list", "--from", "2026-01-01"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(docs.is_empty());
}

#[test]
fn meetings_list_to_before_all_returns_empty() {
    let env = TestEnv::with_fixture();
    // All meetings are after 2025-06-01, so --to 2025-06-01 should return nothing
    let output = env
        .cmd_json()
        .args(["list", "--to", "2025-06-01"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(docs.is_empty());
}

#[test]
fn meetings_search_with_date_filter() {
    let env = TestEnv::with_fixture();
    // Search for "Sprint" in titles, but restrict to after 2025-08-01
    // Only doc-gamma (2025-08-10) should match
    let output = env
        .cmd_json()
        .args([
            "search", "Sprint", "--in", "titles", "--from", "2025-08-01",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["title"], "Gamma Sprint Planning");
}

#[test]
fn transcripts_search_with_date_filter() {
    let env = TestEnv::with_fixture();
    // "kickoff" is in doc-alpha transcript (2025-06-15)
    // With --from 2025-07-01, it should not match
    // Using search with --context to search transcripts
    let output = env
        .cmd_json()
        .args([
            "search", "kickoff", "--context", "2", "--from", "2025-07-01",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // With no matches, both result arrays should be absent or empty
    let transcript_results = result.get("transcript_results")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let text_results = result.get("text_results")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(transcript_results + text_results, 0);
}

#[test]
fn calendars_events_with_date_filter() {
    let env = TestEnv::with_fixture();
    // Filter events to only July
    env.cmd()
        .args([
            "browse", "calendars", "events", "--from", "2025-07-01", "--to", "2025-08-01",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Beta Feature Review"))
        .stdout(predicate::str::contains("Alpha").not())
        .stdout(predicate::str::contains("Gamma").not());
}
