mod common;

use common::TestEnv;
use serde_json::{Value, json};

/// A weekly one-on-one with Robin, twice, and a planning meeting Sam
/// declined. Speaker names appear only in the later one-on-one.
fn team_state() -> String {
    json!({
        "documents": {
            "one-on-one-1": {
                "id": "one-on-one-1",
                "title": "Robin / me",
                "created_at": "2026-03-02T16:00:00.000Z",
                "people": {
                    "creator": {"email": "me@example.com"},
                    "attendees": [{"email": "robin@example.com",
                                   "details": {"person": {"name": {"fullName": "Robin Diaz"}}}}]
                },
                "google_calendar_event": {
                    "recurringEventId": "weekly-robin",
                    "start": {"dateTime": "2026-03-02T10:00:00-06:00"},
                    "end": {"dateTime": "2026-03-02T10:30:00-06:00"},
                    "organizer": {"email": "me@example.com", "self": true},
                    "attendees": [
                        {"email": "me@example.com", "self": true, "responseStatus": "accepted"},
                        {"email": "robin@example.com", "responseStatus": "accepted"}
                    ]
                }
            },
            "one-on-one-2": {
                "id": "one-on-one-2",
                "title": "Robin / me",
                "created_at": "2026-08-03T16:00:00.000Z",
                "google_calendar_event": {"recurringEventId": "weekly-robin"}
            },
            "planning": {
                "id": "planning",
                "title": "Planning",
                "created_at": "2026-03-04T15:00:00.000Z",
                "google_calendar_event": {
                    "attendees": [
                        {"email": "sam@example.com", "responseStatus": "declined"},
                        {"email": "kai@example.com", "responseStatus": "tentative"}
                    ]
                }
            }
        },
        "transcripts": {
            "one-on-one-1": [
                {"id": "a1", "document_id": "one-on-one-1", "start_timestamp": "2026-03-02T16:01:00.000Z",
                 "text": "How was the week?", "source": "microphone"},
                {"id": "a2", "document_id": "one-on-one-1", "start_timestamp": "2026-03-02T16:02:00.000Z",
                 "text": "Busy, but the release went out.", "source": "system"},
                {"id": "a3", "document_id": "one-on-one-1", "start_timestamp": "2026-03-02T16:31:00.000Z",
                 "text": "Same time next week.", "source": "microphone"}
            ],
            "one-on-one-2": [
                {"id": "b1", "document_id": "one-on-one-2", "start_timestamp": "2026-08-03T16:00:00.000Z",
                 "text": "Morning.", "source": "system", "speaker_name": "Robin Diaz"},
                {"id": "b2", "document_id": "one-on-one-2", "start_timestamp": "2026-08-03T16:15:00.000Z",
                 "text": "Bye.", "source": "microphone"}
            ]
        }
    })
    .to_string()
}

fn stats_json(env: &TestEnv, args: &[&str]) -> Value {
    let output = env.cmd_json().arg("stats").args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn overview_counts_meetings_and_spoken_time() {
    let env = TestEnv::with_state(&team_state());

    let overview = stats_json(&env, &[]);

    assert_eq!(
        (
            &overview["meetings"],
            &overview["with_transcript"],
            &overview["spoken_hours"]
        ),
        (&json!(3), &json!(2), &json!(0.75))
    );
}

#[test]
fn overview_dates_the_first_named_speaker() {
    let env = TestEnv::with_state(&team_state());

    let overview = stats_json(&env, &[]);

    assert_eq!(overview["speaker_names_since"], "2026-08-03T16:00:00Z");
}

#[test]
fn meeting_row_carries_schedule_series_and_invitee_responses() {
    let env = TestEnv::with_state(&team_state());

    let rows = stats_json(&env, &["meetings"]);

    assert_eq!(
        (
            &rows[0]["series"],
            &rows[0]["scheduled"],
            &rows[0]["organized_by_me"],
            &rows[0]["invitees"]
        ),
        (
            &json!("weekly-robin"),
            &json!({"start": "2026-03-02T16:00:00Z", "end": "2026-03-02T16:30:00Z"}),
            &json!(true),
            &json!([{"email": "robin@example.com", "name": "Robin Diaz",
                     "response": "accepted", "organizer": false}])
        )
    );
}

#[test]
fn meeting_row_lists_a_line_per_voice() {
    let env = TestEnv::with_state(&team_state());

    let rows = stats_json(&env, &["meetings"]);

    let voices: Vec<(&Value, &Value, &Value)> = rows[2]["transcript"]["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (&v["voice"], &v["name"], &v["chars"]))
        .collect();
    assert_eq!(
        voices,
        vec![
            (&json!("me"), &Value::Null, &json!(4)),
            (&json!("named"), &json!("Robin Diaz"), &json!(8)),
        ]
    );
}

#[test]
fn date_filter_can_follow_the_subcommand() {
    let env = TestEnv::with_state(&team_state());

    let rows = stats_json(
        &env,
        &["meetings", "--from", "2026-03-03", "--to", "2026-04-01"],
    );

    assert_eq!(rows.as_array().unwrap().len(), 1);
}

#[test]
fn empty_database_says_so() {
    let env = TestEnv::with_state(&json!({"documents": {}}).to_string());

    let output = env.cmd().arg("stats").output().unwrap();

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No meetings found.\n"
    );
}

#[test]
fn html_writes_a_page_with_the_meetings_embedded() {
    let env = TestEnv::with_state(&team_state());
    let page = env.dir.path().join("stats.html");

    env.cmd()
        .args(["stats", "--html"])
        .arg(&page)
        .assert()
        .success();

    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.contains(r#""title":"Planning""#));
}

#[test]
fn html_does_not_combine_with_a_subcommand() {
    let env = TestEnv::with_state(&team_state());
    let page = env.dir.path().join("stats.html");

    env.cmd()
        .args(["stats", "--html"])
        .arg(&page)
        .arg("meetings")
        .assert()
        .failure();
}

#[test]
fn time_reports_meetings_and_hours_per_month() {
    let env = TestEnv::with_state(&team_state());

    let report = stats_json(&env, &["time", "--utc"]);

    let months: Vec<(&Value, &Value, &Value)> = report["periods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (&p["start"], &p["meetings"], &p["hours"]))
        .collect();
    assert_eq!(
        months,
        vec![
            (&json!("2026-03-01"), &json!(2), &json!(0.5)),
            (&json!("2026-08-01"), &json!(1), &json!(0.25)),
        ]
    );
}

#[test]
fn time_by_week_starts_weeks_on_monday() {
    let env = TestEnv::with_state(&team_state());

    let report = stats_json(&env, &["time", "--by", "week", "--utc"]);

    assert_eq!(report["periods"][0]["start"], "2026-03-02");
}
