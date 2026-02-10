mod common;

use common::TestEnv;
use predicates::prelude::*;

// --- list ---

#[test]
fn meetings_list_shows_all_meetings() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Alpha Kickoff"))
        .stdout(predicate::str::contains("Beta Feature Review"))
        .stdout(predicate::str::contains("Gamma Sprint Planning"));
}

#[test]
fn meetings_list_json_returns_array() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let docs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(docs.len(), 3);
}

// --- show ---

#[test]
fn meetings_show_by_title_substring() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["show", "Alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Alpha Kickoff"));
}

#[test]
fn meetings_show_by_id() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["show", "doc-beta"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Beta Feature Review"));
}

#[test]
fn meetings_show_json_has_fields() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["show", "doc-alpha"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["id"], "doc-alpha");
    assert_eq!(doc["title"], "Project Alpha Kickoff");
    assert!(doc["summary"].as_str().unwrap().contains("timeline"));
}

#[test]
fn meetings_show_not_found_exits_nonzero() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["show", "nonexistent-meeting-xyz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No meeting found"));
}

#[test]
fn meetings_show_transcript_only() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd()
        .args(["show", "Alpha", "--transcript"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain transcript text
    assert!(stdout.contains("Welcome everyone to the kickoff meeting."));
    assert!(stdout.contains("Today we will discuss the project timeline."));
    // Should NOT contain meeting metadata
    assert!(!stdout.contains("Project Alpha Kickoff"));
}

#[test]
fn meetings_show_notes_only() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd()
        .args(["show", "Alpha", "--notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain notes text
    assert!(stdout.contains("Discussed the project timeline and milestones for Q3 delivery."));
    // Should NOT contain meeting metadata like title as a heading
    assert!(!stdout.contains("Title:"));
}

#[test]
fn meetings_show_transcript_json() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["show", "Alpha", "--transcript"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // Should have transcript array
    assert!(json["transcript"].is_array());
    let transcript = json["transcript"].as_array().unwrap();
    assert_eq!(transcript.len(), 5);
    // Should NOT have notes
    assert!(json.get("notes_plain").is_none());
}

#[test]
fn meetings_show_notes_json() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["show", "Alpha", "--notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // Should have both notes formats
    assert!(json["notes_plain"]
        .as_str()
        .unwrap()
        .contains("Discussed the project timeline"));
    assert!(json["notes_markdown"]
        .as_str()
        .unwrap()
        .contains("**project timeline**"));
    // Should NOT have transcript
    assert!(json.get("transcript").is_none());
}

#[test]
fn meetings_show_transcript_and_notes() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd()
        .args(["show", "Alpha", "--transcript", "--notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain notes
    assert!(stdout.contains("Discussed the project timeline and milestones for Q3 delivery."));
    // Should contain separator
    assert!(stdout.contains("---"));
    // Should contain transcript
    assert!(stdout.contains("Welcome everyone to the kickoff meeting."));
}

#[test]
fn meetings_show_transcript_no_transcript_errors() {
    let env = TestEnv::with_fixture();
    // doc-gamma has no transcript in fixture
    env.cmd()
        .args(["show", "Gamma", "--transcript"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No transcript available"));
}

// --- search ---

#[test]
fn meetings_search_by_title() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["search", "Alpha", "--in", "titles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Alpha Kickoff"));
}

#[test]
fn meetings_search_by_notes() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["search", "benchmarks", "--in", "notes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Beta Feature Review"));
}

#[test]
fn meetings_search_by_transcripts() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["search", "prototype", "--in", "transcripts"])
        .assert()
        .success()
        .stdout(predicate::str::contains("doc-alpha").or(predicate::str::contains("doc-beta")));
}

// --- search with context (transcript search) ---

#[test]
fn transcripts_search_finds_match() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["search", "deadline", "--context", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deadline"));
}

#[test]
fn transcripts_search_json_has_context_window() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["search", "deadline", "--context", "1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let windows = result["transcript_results"].as_array().unwrap();
    assert!(!windows.is_empty());
    let window = &windows[0];
    assert!(window["matched"]["text"].as_str().unwrap().contains("deadline"));
}

#[test]
fn transcripts_search_within_meeting() {
    let env = TestEnv::with_fixture();
    // Search for "prototype" but restrict to doc-beta
    env.cmd()
        .args(["search", "prototype", "--context", "2", "--meeting", "Beta"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prototype").or(predicate::str::contains("staging")));
}

// --- browse people list ---

#[test]
fn people_list_shows_all() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "people", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice Johnson"))
        .stdout(predicate::str::contains("Bob Smith"))
        .stdout(predicate::str::contains("Carol Williams"));
}

#[test]
fn people_list_filter_by_company() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "people", "list", "--company", "Widgets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Carol Williams"))
        .stdout(predicate::str::contains("Alice Johnson").not());
}

#[test]
fn people_list_json() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["browse", "people", "list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let people: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(people.len(), 3);
}

// --- browse people show ---

#[test]
fn people_show_by_name() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "people", "show", "Alice"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice Johnson"));
}

#[test]
fn people_show_by_email() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "people", "show", "bob@example.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bob Smith"));
}

// --- with (people meetings) ---

#[test]
fn people_meetings_returns_correct_meetings() {
    let env = TestEnv::with_fixture();
    // Alice is in doc-alpha and doc-gamma
    env.cmd()
        .args(["with", "Alice"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Alpha Kickoff"))
        .stdout(predicate::str::contains("Gamma Sprint Planning"));
}

// --- browse calendars list ---

#[test]
fn calendars_list_shows_all() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "calendars", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("alice@example.com"))
        .stdout(predicate::str::contains("Team Calendar"));
}

#[test]
fn calendars_list_json() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["browse", "calendars", "list"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let cals: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cals.len(), 2);
}

// --- browse calendars events ---

#[test]
fn calendars_events_shows_events() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "calendars", "events"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Alpha Kickoff"))
        .stdout(predicate::str::contains("Beta Feature Review"));
}

// --- browse templates list ---

#[test]
fn templates_list_shows_all() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "templates", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Meeting Notes"))
        .stdout(predicate::str::contains("Daily Standup"));
}

#[test]
fn templates_list_filter_by_category() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "templates", "list", "--category", "agile"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Daily Standup"))
        .stdout(predicate::str::contains("Meeting Notes").not());
}

// --- browse templates show ---

#[test]
fn templates_show_by_title() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "templates", "show", "Meeting Notes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Meeting Notes"));
}

// --- browse recipes list ---

#[test]
fn recipes_list_shows_all() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "recipes", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Summarize meeting notes"))
        .stdout(predicate::str::contains("Extract action items"));
}

#[test]
fn recipes_list_filter_by_visibility() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "recipes", "list", "--visibility", "public"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Summarize meeting notes"))
        .stdout(predicate::str::contains("Extract action items").not());
}

// --- browse recipes show ---

#[test]
fn recipes_show_by_slug() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "recipes", "show", "meeting-summarizer"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Summarize meeting notes"));
}

#[test]
fn recipes_show_json_has_config() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["browse", "recipes", "show", "meeting-summarizer"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gpt-4"));
    assert!(stdout.contains("Summarize meeting notes"));
}

// --- chat_url in meetings show ---

#[test]
fn meetings_show_displays_chat_url() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd()
        .args(["show", "Alpha"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("notes.granola.ai/t/alpha-meeting-123"),
        "TTY output should contain the chat URL"
    );
}

#[test]
fn meetings_show_json_includes_chat_url() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["show", "doc-alpha"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let panels = doc["panels"].as_array().expect("should have panels array");
    assert!(!panels.is_empty());
    assert_eq!(
        panels[0]["chat_url"].as_str(),
        Some("https://notes.granola.ai/t/alpha-meeting-123")
    );
}

#[test]
fn meetings_show_json_chat_url_null_when_absent() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["show", "doc-beta"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let panels = doc["panels"].as_array().expect("should have panels array");
    assert!(!panels.is_empty());
    assert!(panels[0]["chat_url"].is_null());
}
