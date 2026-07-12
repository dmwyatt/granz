mod common;

use common::TestEnv;
use predicates::prelude::*;

// --- --context: card expansion on grep cards ---

#[test]
fn context_search_single_word() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["grep", "kickoff", "--context", "2", "--in", "transcripts"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    assert!(!meetings.is_empty());

    let snippet = meetings[0]["matches"][0]["snippet"].as_str().unwrap();
    assert!(snippet.to_lowercase().contains("kickoff"));
}

#[test]
fn context_search_phrase() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["grep", "resource allocation", "--context", "2", "--in", "transcripts"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    assert!(!meetings.is_empty());

    let snippet = meetings[0]["matches"][0]["snippet"].as_str().unwrap();
    assert!(snippet.contains("resource allocation"));
}

#[test]
fn context_search_no_match_returns_empty() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["grep", "xyzzyplughnotaword", "--context", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total_meetings"], 0);
}

#[test]
fn context_search_has_before_and_after_units() {
    let env = TestEnv::with_fixture();
    // "timeline" is in utt-a2 (index 1) of doc-alpha's transcript: 1
    // utterance before, 2 after at context 2. Evidence priority puts the
    // panel and notes sites first, so find the transcript match.
    let output = env
        .cmd_json()
        .args(["grep", "timeline", "--context", "2", "--matches", "5"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let matches = result["meetings"][0]["matches"].as_array().unwrap();
    let m = matches
        .iter()
        .find(|m| m["source"] == "transcript")
        .unwrap_or_else(|| panic!("no transcript match in: {result}"));
    assert_eq!(m["context_before"].as_array().unwrap().len(), 1);
    assert_eq!(m["context_after"].as_array().unwrap().len(), 2);
    // Transcript neighbors carry speaker and timestamp.
    assert_eq!(m["context_after"][0]["speaker"], "other");
    assert!(m["context_after"][0]["timestamp"].is_string());
}

#[test]
fn context_search_within_specific_meeting() {
    let env = TestEnv::with_fixture();

    // "latency" only appears in doc-beta transcript
    let output = env
        .cmd_json()
        .args(["grep", "latency", "--context", "2", "--meeting", "Beta"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    assert!(!meetings.is_empty());
    assert_eq!(meetings[0]["id"], "doc-beta");
}

#[test]
fn context_search_restricted_to_wrong_meeting_returns_empty() {
    let env = TestEnv::with_fixture();

    // "latency" is only in doc-beta, so searching within "Alpha" should yield nothing
    let output = env
        .cmd_json()
        .args(["grep", "latency", "--context", "2", "--meeting", "Alpha"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total_meetings"], 0);
}

// --- FTS5 notes search (via search --in notes) ---

#[test]
fn meetings_search_notes_fts() {
    let env = TestEnv::with_fixture();
    // "milestones" appears in doc-alpha notes
    env.cmd()
        .args(["grep", "milestones", "--in", "notes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("doc-alpha").or(predicate::str::contains("Alpha")));
}

#[test]
fn meetings_search_notes_no_match() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["grep", "xyzzynotaword", "--in", "notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total_meetings"], 0);
    assert!(result["meetings"].as_array().unwrap().is_empty());
}

#[test]
fn grep_json_is_shaped_with_evidence() {
    let env = TestEnv::with_fixture();
    // "milestones" appears in doc-alpha notes; grep renders the same shaped
    // cards as search.
    let output = env
        .cmd_json()
        .args(["grep", "milestones", "--in", "notes"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    assert_eq!(meetings.len(), 1);

    let m = &meetings[0];
    assert_eq!(m["id"], "doc-alpha");
    assert!(m["score"].is_null(), "grep results carry no rerank score");
    let signals: Vec<&str> =
        m["signals"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    assert!(signals.contains(&"keyword"));
    assert!(!signals.contains(&"semantic"));

    assert!(m["total_matches"].as_u64().unwrap() >= 1);
    let snippet = m["matches"][0]["snippet"].as_str().unwrap();
    assert!(snippet.to_lowercase().contains("milestones"));
    assert!(!m["matches"][0]["highlights"].as_array().unwrap().is_empty());
}

#[test]
fn grep_matches_flag_shows_more_snippets() {
    let env = TestEnv::with_fixture();
    // "timeline" has three match sites in doc-alpha: a panel section, a
    // notes paragraph, and a transcript utterance.
    let output = env
        .cmd_json()
        .args(["grep", "timeline", "--matches", "3"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    let alpha = meetings.iter().find(|m| m["id"] == "doc-alpha").unwrap();
    assert!(
        alpha["matches"].as_array().unwrap().len() > 1,
        "expected multiple excerpted matches for doc-alpha, got: {alpha}"
    );
}

#[test]
fn grep_tty_shows_match_evidence() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["grep", "milestones", "--in", "notes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("your notes"))
        .stdout(predicate::str::contains("milestones"));
}

// --- Multi-target search ---

#[test]
fn meetings_search_multiple_targets() {
    let env = TestEnv::with_fixture();
    // "prototype" appears in transcript of doc-alpha (utt-a3) and doc-beta (utt-b3)
    // Searching both titles and transcripts
    env.cmd()
        .args(["grep", "prototype", "--in", "titles,transcripts"])
        .assert()
        .success();
}

// --- Limit flag ---

#[test]
fn grep_respects_limit() {
    let env = TestEnv::with_fixture();
    // "prototype" matches in both doc-alpha and doc-beta transcripts (2 meetings).
    // --limit 1 should return only 1 result.
    let output = env
        .cmd_json()
        .args(["grep", "prototype", "--in", "transcripts", "--limit", "1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["meetings"].as_array().unwrap().len(), 1);
    assert!(result["total_meetings"].as_u64().unwrap() >= 2);
}

#[test]
fn grep_limit_zero_returns_all() {
    let env = TestEnv::with_fixture();
    // --limit 0 means no limit, should return all matches
    let output = env
        .cmd_json()
        .args(["grep", "prototype", "--in", "transcripts", "--limit", "0"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["meetings"].as_array().unwrap().len() >= 2);
}

#[test]
fn grep_tty_header_reports_complete_count_when_limited() {
    let env = TestEnv::with_fixture();
    // "prototype" matches doc-alpha and doc-beta transcripts; the header
    // must report the complete count (grep's contract), not the page size.
    env.cmd()
        .args(["grep", "prototype", "--in", "transcripts", "--limit", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Found 2 meeting(s) matching \"prototype\" (showing 1):",
        ))
        .stdout(predicate::str::contains("Use --limit 0 to show all 2 results."));
}

#[test]
fn grep_tty_header_without_truncation_omits_showing() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["grep", "prototype", "--in", "transcripts"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Found 2 meeting(s) matching \"prototype\":"))
        .stdout(predicate::str::contains("(showing").not());
}

// --- Speaker filter: restricts match evidence to a speaker's utterances ---

#[test]
fn speaker_filter_keeps_meetings_with_matching_utterances() {
    let env = TestEnv::with_fixture();
    // "prototype" appears in system ("other") utterances of doc-alpha and
    // doc-beta; both survive the filter and the evidence is the utterance.
    let output = env
        .cmd_json()
        .args(["grep", "prototype", "--speaker", "other"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let meetings = result["meetings"].as_array().unwrap();
    assert_eq!(meetings.len(), 2, "got: {result}");
    for m in meetings {
        assert_eq!(m["matches"][0]["source"], "transcript");
        assert_eq!(m["matches"][0]["speaker"], "other");
    }
}

#[test]
fn speaker_filter_drops_meetings_without_attributable_evidence() {
    let env = TestEnv::with_fixture();
    // "milestones" matches doc-alpha only in notes and a panel section.
    // With a speaker filter only transcript evidence counts, so the meeting
    // drops out entirely instead of showing unattributable matches.
    let output = env
        .cmd_json()
        .args(["grep", "milestones", "--speaker", "other"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total_meetings"], 0, "got: {result}");
}

#[test]
fn speaker_filter_me_matches_nothing_in_all_system_fixture() {
    let env = TestEnv::with_fixture();
    let output = env
        .cmd_json()
        .args(["grep", "prototype", "--speaker", "me"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["total_meetings"], 0, "got: {result}");
}

#[test]
fn grep_speaker_with_in_excluding_transcripts_errors() {
    let env = TestEnv::with_fixture();
    // --speaker matches transcript utterances; an --in list without
    // transcripts leaves it nothing to match, so grep refuses to guess.
    env.cmd()
        .args(["grep", "milestones", "--speaker", "me", "--in", "notes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("transcripts"));
}

#[test]
fn context_search_limit_counts_meetings() {
    let env = TestEnv::with_fixture();
    // "prototype" matches in both doc-alpha and doc-beta transcripts.
    let output = env
        .cmd_json()
        .args(["grep", "prototype", "--context", "1", "--limit", "1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["meetings"].as_array().unwrap().len(), 1);
    assert_eq!(result["total_meetings"], 2);
}
