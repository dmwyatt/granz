mod common;

use predicates::prelude::*;

use common::TestEnv;

#[test]
fn empty_state_returns_empty_results() {
    let env = TestEnv::with_state(r#"{"events":[],"people":[],"calendars":[]}"#);

    env.cmd()
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("["));
}

#[test]
fn people_show_nonexistent_errors() {
    let env = TestEnv::with_fixture();
    env.cmd()
        .args(["browse", "people", "show", "Zephyr McNoexist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No person found"));
}

#[test]
fn invalid_subcommand_shows_help() {
    let mut cmd = assert_cmd::cargo_bin_cmd!("grans");
    cmd.arg("nonexistent-subcommand");

    cmd.assert().failure();
}
