//! A start gate: closing it keeps git from starting, and leaves running git alone.

mod common;

use std::time::Duration;

use cairn_write::{Invocation, JobClass, RunError, StartGate};
use common::{Release, Repo, blocking_alias, git, local_write};

fn commit_through(repo: &Repo, gate: &StartGate) -> Result<cairn_write::Output, RunError> {
    git().run(
        &Invocation::new(JobClass::LocalWrite, &repo.path)
            .args([
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "through the gate",
            ])
            .through(gate),
    )
}

fn has_a_commit(repo: &Repo) -> bool {
    !local_write(&repo.path, &["rev-parse", "--verify", "-q", "HEAD"], None)
        .stdout
        .is_empty()
}

#[test]
fn a_closed_gate_keeps_git_from_starting() {
    let repo = Repo::new();
    let gate = StartGate::new();

    gate.close();
    // Seen red: a gate that ignores being closed lets the commit run. Control:
    // `an_open_gate_lets_git_run`.
    let closed = commit_through(&repo, &gate);
    assert!(matches!(closed, Err(RunError::GateClosed)), "{closed:?}");
    assert!(!has_a_commit(&repo));
}

#[test]
fn an_open_gate_lets_git_run() {
    let repo = Repo::new();
    commit_through(&repo, &StartGate::new())
        .and_then(|out| out.success())
        .expect("the commit runs");
    assert!(has_a_commit(&repo));
}

#[test]
fn closing_the_gate_leaves_running_git_alone() {
    let repo = Repo::new();
    let release = repo.path.join("release");
    let _release = Release(&release);
    let gate = StartGate::new();
    let invocation = Invocation::new(JobClass::LocalWrite, &repo.path)
        .args(["-c", &blocking_alias(&release), "block"])
        .through(&gate);

    let out = std::thread::scope(|scope| {
        let (tx, waiting) = std::sync::mpsc::channel();
        let run = scope.spawn(move || {
            git().run_streaming(&invocation, |chunk| {
                if String::from_utf8_lossy(chunk).contains("waiting") {
                    let _ = tx.send(());
                }
            })
        });
        waiting
            .recv_timeout(Duration::from_secs(30))
            .expect("the alias started");
        // Returns while git is still blocked: a close that waited for git would deadlock
        // here, since only the write below releases it.
        // Seen red: a run that reports the closed gate afterwards fails `expect` below.
        gate.close();
        std::fs::write(&release, "").unwrap();
        run.join().unwrap()
    })
    .and_then(|out| out.success())
    .expect("git runs to its end");
    assert!(
        out.stderr_text().contains("released"),
        "{}",
        out.stderr_text()
    );
}
