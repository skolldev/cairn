//! What a result records, and what makes the harness refuse to produce one.

use cairn_budget::report::{Environment, Row, render, verdict_exit_code};
use cairn_budget::spawn;
use cairn_budget::stats::{Budget, judge};

fn environment() -> Environment {
    Environment {
        machine: "Mac15,8 Apple M3 Max".to_owned(),
        os: "macOS 26.1".to_owned(),
        git_version: "2.39.3".to_owned(),
        git_path: "/usr/bin/git".to_owned(),
        rust_version: "rustc 1.98.1".to_owned(),
        cairn_commit: "abc123".to_owned(),
        frame_source: "Metal presentedTime".to_owned(),
    }
}

fn row(samples: f64) -> Row {
    let runs = vec![vec![samples; 20], vec![samples; 20], vec![samples; 20]];
    Row {
        name: "External change, file save".to_owned(),
        fixture: "linux-graph".to_owned(),
        fixture_head: "05441d69".to_owned(),
        summary: judge(&Budget::Latency { limit_ms: 100.0 }, &runs),
        runs,
    }
}

#[test]
fn a_result_records_where_it_came_from() {
    let text = render(&environment(), &[row(40.0)]);
    for fact in [
        "Mac15,8 Apple M3 Max",
        "macOS 26.1",
        "2.39.3",
        "/usr/bin/git",
        "rustc 1.98.1",
        "abc123",
        "Metal presentedTime",
        "linux-graph",
        "05441d69",
    ] {
        assert!(text.contains(fact), "no {fact} in:\n{text}");
    }
}

#[test]
fn a_result_records_the_statistics_the_rules_name_and_every_sample() {
    let mut row = row(40.0);
    row.runs[1][3] = 91.5;
    row.summary = judge(&Budget::Latency { limit_ms: 100.0 }, &row.runs);
    let text = render(&environment(), &[row]);
    assert!(text.contains("| 40.0 |"), "p95:\n{text}");
    assert!(text.contains("| 91.5 |"), "worst sample:\n{text}");
    assert!(text.contains("| Pass |"), "verdict:\n{text}");
    assert!(text.contains("91.5, "), "raw samples:\n{text}");
}

#[test]
fn only_a_full_pass_exits_zero() {
    assert_eq!(verdict_exit_code(&[row(40.0)]), 0);
    assert_eq!(verdict_exit_code(&[row(40.0), row(150.0)]), 1, "a miss");
    let mut invalid = row(40.0);
    invalid.runs.pop();
    invalid.summary = judge(&Budget::Latency { limit_ms: 100.0 }, &invalid.runs);
    assert_eq!(verdict_exit_code(&[invalid]), 1, "an invalid result");
    assert_eq!(verdict_exit_code(&[]), 1, "nothing measured");
}

#[test]
fn the_command_refuses_to_measure_on_a_ci_runner() {
    let out = spawn::tool(env!("CARGO_BIN_EXE_cairn-budget"))
        .unwrap()
        // No such binary, so that without the refusal this fails at once rather than measuring.
        .args(["run", "--only", "launch", "--cairn", "/nonexistent/cairn"])
        .env("CI", "true")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("CI runner"), "{stderr}");
}

#[test]
fn the_harness_starts_anything_but_git() {
    assert!(spawn::tool("git").is_err());
    assert!(spawn::tool("/usr/bin/git").is_err());
    assert!(spawn::tool(r"C:\Program Files\Git\cmd\git.exe").is_err());
    assert!(spawn::tool("PresentMon.exe").is_ok());
}
