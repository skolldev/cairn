//! Stopping a Network Job or a Read Job through the runner: a Local write invocation can't
//! be stopped (ADR 0002), and stopping kills git together with everything it started.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use cairn_write::{Invocation, JobClass, RunError, Stop};
use common::{Release, Repo, blocking_alias, forward_slashes, git};

#[test]
fn stopping_a_network_invocation_kills_git_and_what_it_started() {
    stopping_kills_git_and_what_it_started(JobClass::Network);
}

/// A Read Job is stopped when a newer one Supersedes it. It takes no lock, so killing it
/// leaves nothing behind.
#[test]
fn stopping_a_read_invocation_kills_git_and_what_it_started() {
    stopping_kills_git_and_what_it_started(JobClass::Read);
}

fn stopping_kills_git_and_what_it_started(class: JobClass) {
    let repo = Repo::new();
    let release = repo.path.join("release");
    let _release = Release(&release);
    let stop = Stop::new();
    let invocation = Invocation::new(class, &repo.path)
        .args(["-c", &blocking_alias(&release), "block"])
        .stop_on(&stop);

    let started = Instant::now();
    let result = std::thread::scope(|scope| {
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
        stop.stop();
        run.join().unwrap()
    });

    match result {
        Err(RunError::Stopped { stderr }) => {
            // Whole lines: git may report the killed alias as `error: <its command> died of
            // signal 15`, and that command contains `echo waiting` and `echo released` (#113).
            let stderr = String::from_utf8_lossy(&stderr);
            let printed = |line: &str| stderr.lines().any(|l| l.trim_end() == line);
            assert!(printed("waiting"), "{stderr}");
            assert!(!printed("released"), "{stderr}");
        }
        other => panic!("the invocation must report it was stopped: {other:?}"),
    }
    // Waiting on the shell would mean it outlived git and kept the pipe open.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the shell git started was not stopped with it"
    );
}

/// An alias that creates `marker`, so a test can tell whether git ran.
fn marking_alias(marker: &Path) -> String {
    let marker = forward_slashes(marker);
    format!("alias.mark=!touch '{marker}'")
}

#[test]
fn a_stop_asked_for_before_git_starts_means_git_never_runs() {
    let repo = Repo::new();
    let marker = repo.path.join("ran");
    let stop = Stop::new();
    stop.stop();

    let result = git().run(
        &Invocation::new(JobClass::Network, &repo.path)
            .args(["-c", &marking_alias(&marker), "mark"])
            .stop_on(&stop),
    );

    assert!(
        matches!(result, Err(RunError::Stopped { .. })),
        "{result:?}"
    );
    assert!(!marker.exists(), "git ran although it was already stopped");
    // Control: without the stop, the same alias leaves the marker.
    common::ok(
        git()
            .run(&Invocation::new(JobClass::Network, &repo.path).args([
                "-c",
                &marking_alias(&marker),
                "mark",
            ]))
            .unwrap(),
    );
    assert!(marker.exists());
}

#[test]
fn a_network_invocation_that_finishes_before_the_stop_reports_its_own_outcome() {
    let repo = Repo::new();
    let stop = Stop::new();
    let out = git()
        .run(
            &Invocation::new(JobClass::Network, &repo.path)
                .args(["config", "--get", "core.longpaths"])
                .stop_on(&stop),
        )
        .expect("git runs");
    stop.stop();
    assert_eq!(out.stdout, b"true\n");
}

#[test]
fn the_runner_refuses_to_make_a_local_write_stoppable() {
    let repo = Repo::new();
    let marker = repo.path.join("ran");
    let result = git().run(
        &Invocation::new(JobClass::LocalWrite, &repo.path)
            .args(["-c", &marking_alias(&marker), "mark"])
            .stop_on(&Stop::new()),
    );
    assert!(
        matches!(
            result,
            Err(RunError::NotStoppable {
                class: JobClass::LocalWrite
            })
        ),
        "{result:?}"
    );
    assert!(!marker.exists(), "git ran");
}
