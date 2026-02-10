mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn people_meetings_alice_has_alpha_and_gamma() {
    let env = TestEnv::with_fixture();
    // Alice is creator/attendee of doc-alpha and doc-gamma
    let output = env
        .cmd_json()
        .args(["with", "Alice"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 2);

    let titles: Vec<&str> = docs
        .iter()
        .map(|d| d["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Project Alpha Kickoff"));
    assert!(titles.contains(&"Gamma Sprint Planning"));
    assert!(!titles.contains(&"Beta Feature Review"));
}

#[test]
fn people_meetings_bob_has_alpha_and_beta() {
    let env = TestEnv::with_fixture();
    // Bob is attendee of doc-alpha and creator of doc-beta
    let output = env
        .cmd_json()
        .args(["with", "Bob"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 2);

    let titles: Vec<&str> = docs
        .iter()
        .map(|d| d["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Project Alpha Kickoff"));
    assert!(titles.contains(&"Beta Feature Review"));
    assert!(!titles.contains(&"Gamma Sprint Planning"));
}

#[test]
fn people_meetings_carol_has_beta_and_gamma() {
    let env = TestEnv::with_fixture();
    // Carol is attendee of doc-beta and doc-gamma
    let output = env
        .cmd_json()
        .args(["with", "Carol"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 2);

    let titles: Vec<&str> = docs
        .iter()
        .map(|d| d["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Beta Feature Review"));
    assert!(titles.contains(&"Gamma Sprint Planning"));
    assert!(!titles.contains(&"Project Alpha Kickoff"));
}

#[test]
fn people_meetings_by_email() {
    let env = TestEnv::with_fixture();
    // Should also work by email fragment
    env.cmd()
        .args(["with", "carol@widgets.io"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Beta Feature Review"))
        .stdout(predicate::str::contains("Gamma Sprint Planning"));
}

#[test]
fn people_meetings_nonexistent_person_errors() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["with", "Zephyr McNoexist"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().or(predicate::str::contains("No meetings")));
}

#[test]
fn people_meetings_json_structure() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["with", "Alice"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();

    for doc in &docs {
        assert!(doc["id"].is_string());
        assert!(doc["title"].is_string());
        assert!(doc["created_at"].is_string());
    }
}
