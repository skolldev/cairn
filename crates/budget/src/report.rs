//! What a harness run leaves behind: every number, and where it came from.

use std::fmt::Write as _;

use crate::stats::{Summary, Verdict};

/// What a number depends on, recorded with every result.
#[derive(Clone, Debug)]
pub struct Environment {
    pub machine: String,
    pub os: String,
    pub git_version: String,
    pub git_path: String,
    pub rust_version: String,
    pub cairn_commit: String,
    /// What the frame times come from: Metal's presented times, or PresentMon and its version.
    pub frame_source: String,
}

/// One Budget's result.
#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub fixture: String,
    pub fixture_head: String,
    pub summary: Summary,
    /// Every sample, per run, in milliseconds.
    pub runs: Vec<Vec<f64>>,
}

fn verdict_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Pass => "Pass".to_owned(),
        Verdict::Miss(why) => format!("Miss: {why}"),
        Verdict::Invalid(why) => format!("Invalid: {why}"),
    }
}

/// The result as Markdown.
pub fn render(environment: &Environment, rows: &[Row]) -> String {
    let e = environment;
    let mut text = String::from("# Budget harness result\n\n");
    let _ = writeln!(text, "- Machine: {}", e.machine);
    let _ = writeln!(text, "- OS: {}", e.os);
    let _ = writeln!(text, "- git: {} at {}", e.git_version, e.git_path);
    let _ = writeln!(text, "- Rust: {}", e.rust_version);
    let _ = writeln!(text, "- Cairn: {}", e.cairn_commit);
    let _ = writeln!(text, "- Frame source: {}", e.frame_source);
    text.push_str(
        "\nNumbers count only from macOS and the physical Windows PC, never from a VM or a CI \
         runner.\n\n",
    );
    text.push_str(
        "| Budget | Fixture | Fixture HEAD | Samples per run | Budget's statistic, ms | \
         p95 of worst run, ms | Worst sample, ms | Verdict |\n",
    );
    text.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for row in rows {
        let s = &row.summary;
        let _ = writeln!(
            text,
            "| {} | {} | {} | {:?} | {:.1} | {:.1} | {:.1} | {} |",
            row.name,
            row.fixture,
            row.fixture_head,
            s.samples_per_run,
            s.statistic_ms,
            s.p95_ms,
            s.worst_sample_ms,
            verdict_text(&s.verdict),
        );
    }
    text.push_str("\n## Samples, ms\n");
    for row in rows {
        let _ = writeln!(text, "\n### {}\n", row.name);
        for (index, run) in row.runs.iter().enumerate() {
            let samples: Vec<String> = run.iter().map(|s| format!("{s:.1}")).collect();
            let _ = writeln!(text, "- Run {}: {}", index + 1, samples.join(", "));
        }
    }
    text
}

/// 0 only if something was measured and everything passed.
pub fn verdict_exit_code(rows: &[Row]) -> i32 {
    let passed = !rows.is_empty() && rows.iter().all(|r| r.summary.verdict == Verdict::Pass);
    i32::from(!passed)
}
