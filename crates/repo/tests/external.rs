//! External changes narrow the Unstaged and Untracked reads to the paths the watcher saw
//! (#40), and the lists they publish still match `git status` after every kind of change.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use cairn_repo::{Intent, Snapshot};
use common::{Harness, changes, git, git_status, open, path, small_fixture_clone, touch};

/// What Cairn shows for Unstaged and Untracked, as git renders them.
#[derive(Debug, Default, PartialEq, Eq)]
struct Shown {
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

/// Drains until the newest Unstaged and Untracked Snapshots match what git reports now.
/// A narrowed read that missed a change never matches, and this times out.
fn wait_for_git(harness: &Harness, repo: &Path, shown: &mut Shown, after: &str) {
    let git = git_status(repo);
    let expected = Shown {
        unstaged: git.unstaged,
        untracked: git.untracked,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while *shown != expected {
        for snapshot in harness.service.drain() {
            match snapshot {
                Snapshot::Unstaged(u) => shown.unstaged = changes(&u),
                Snapshot::Untracked(u) => {
                    shown.untracked = u.result.as_ref().unwrap().iter().map(path).collect()
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "after {after}, Cairn shows {shown:#?} but git reports {expected:#?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write(repo: &Path, relative: &str, content: &str) {
    let file = repo.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
}

#[test]
fn status_matches_git_after_each_kind_of_external_change() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.clone()));
    let mut shown = Shown::default();
    wait_for_git(&harness, &repo, &mut shown, "the Open");

    // Seen red: this passed before reads were narrowed, so each narrowing was broken in turn.
    // With Untracked's region taken as the changed path itself, the burst goes red. With
    // Untracked keeping the entries its regions cover, the removed directory does. With
    // Unstaged keeping the entries its paths cover, the undone save does.
    let readme = std::fs::read_to_string(repo.join("README.md")).unwrap();
    write(&repo, "README.md", "saved outside Cairn\n");
    wait_for_git(&harness, &repo, &mut shown, "a tracked file saved");

    // Back to what's committed, at another mtime: only stat-dirty, so not listed.
    write(&repo, "README.md", &readme);
    touch(&repo.join("README.md"));
    wait_for_git(&harness, &repo, &mut shown, "a save undone");

    std::fs::remove_file(repo.join("COPYING")).unwrap();
    wait_for_git(&harness, &repo, &mut shown, "a tracked file deleted");

    write(&repo, "crates/new.rs", "new\n");
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "a file added to a tracked directory",
    );

    write(&repo, "fresh/deeper/one.txt", "1\n");
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "a file added in new directories",
    );
    write(&repo, "fresh/two.txt", "2\n");
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "a second file in the new directory",
    );
    std::fs::remove_dir_all(repo.join("fresh")).unwrap();
    wait_for_git(&harness, &repo, &mut shown, "the new directory removed");

    // An untracked file beside a directory's only tracked files, which then go.
    write(&repo, "crates/matcher/scratch.txt", "scratch\n");
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "an untracked file beside tracked ones",
    );
    std::fs::remove_dir_all(repo.join("crates/matcher/src")).unwrap();
    std::fs::remove_file(repo.join("crates/matcher/Cargo.toml")).unwrap();
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "a directory's tracked files deleted",
    );

    std::fs::rename(repo.join("crates/cli"), repo.join("crates/cli2")).unwrap();
    wait_for_git(&harness, &repo, &mut shown, "a tracked directory renamed");

    // A burst, as a build or a checkout writes: many events, each read narrowed or not.
    for i in 0..200 {
        write(&repo, &format!("crates/burst/{i}.txt"), "burst\n");
    }
    for file in std::fs::read_dir(repo.join("crates/regex/src")).unwrap() {
        let file = file.unwrap().path();
        if file.is_file() {
            std::fs::write(&file, "rewritten\n").unwrap();
        }
    }
    wait_for_git(&harness, &repo, &mut shown, "a burst of writes");

    // The index changes too: everything is read again, and still matches.
    git(&repo, &["add", "--", "crates/new.rs", "README.md"]);
    wait_for_git(&harness, &repo, &mut shown, "an index write");
    write(&repo, "crates/new.rs", "changed after staging\n");
    wait_for_git(&harness, &repo, &mut shown, "a staged file saved again");
}

#[test]
fn a_change_to_excludes_reads_untracked_again() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    write(&repo, "notes/todo.txt", "todo\n");
    write(&repo, "build.log", "log\n");
    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.clone()));
    let mut shown = Shown::default();
    wait_for_git(&harness, &repo, &mut shown, "the Open");

    // Nothing under `notes/` or the log changes, only what ignores them.
    // Seen red: with a `.gitignore` change narrowing Untracked instead of reading it all,
    // the first change goes red.
    let mut ignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap_or_default();
    ignore.push_str("\n/notes/\n*.log\n");
    write(&repo, ".gitignore", &ignore);
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "a .gitignore that ignores them",
    );

    write(&repo, ".git/info/exclude", "");
    let ignore = ignore.replace("\n/notes/\n*.log\n", "\n");
    write(&repo, ".gitignore", &ignore);
    wait_for_git(&harness, &repo, &mut shown, "that .gitignore undone");

    write(&repo, ".git/info/exclude", "notes/\n");
    wait_for_git(
        &harness,
        &repo,
        &mut shown,
        "info/exclude ignoring a directory",
    );
}

#[test]
fn a_file_saved_outside_the_service_does_not_read_staged_again() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    let opened = open(&harness, &repo);
    let mut shown = Shown::default();

    write(&repo, "README.md", "saved outside Cairn\n");
    wait_for_git(&harness, &repo, &mut shown, "a tracked file saved");

    // Marker: an Open asks every Slot again. A fresh service numbers a Slot's reads 1, 2, ...,
    // so its Staged answer is the next generation only if the save asked for none.
    harness.service.send(Intent::Open(repo.clone()));
    let mut staged = None;
    harness.drain_until(|s| match s {
        Snapshot::Staged(s) if s.generation > opened.staged => {
            staged = Some(s.generation.get());
            true
        }
        _ => false,
    });
    assert_eq!(
        staged,
        Some(opened.staged.get() + 1),
        "a saved file can't change HEAD vs index"
    );
}
